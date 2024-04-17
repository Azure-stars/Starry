//! Startup process for monolithic kernel.

#![cfg_attr(not(test), no_std)]

#[macro_use]
extern crate axlog;
extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(all(target_os = "none", not(test)))]
mod lang_items;

use axerrno::{LinuxError, LinuxResult};
use core::sync::atomic::{AtomicUsize, Ordering};
use fork::{user_mode_thread, CloneFlags};
use axtype::DtbInfo;
use axhal::mem::{phys_to_virt, memory_regions};
use preempt_guard::NoPreempt;

#[cfg(feature = "smp")]
mod mp;

static INITED_CPUS: AtomicUsize = AtomicUsize::new(0);

fn is_init_ok() -> bool {
    INITED_CPUS.load(Ordering::Acquire) == axconfig::SMP
}

const LOGO: &str = r#"
       d8888                            .d88888b.   .d8888b.
      d88888                           d88P" "Y88b d88P  Y88b
     d88P888                           888     888 Y88b.
    d88P 888 888d888  .d8888b  .d88b.  888     888  "Y888b.
   d88P  888 888P"   d88P"    d8P  Y8b 888     888     "Y88b.
  d88P   888 888     888      88888888 888     888       "888
 d8888888888 888     Y88b.    Y8b.     Y88b. .d88P Y88b  d88P
d88P     888 888      "Y8888P  "Y8888   "Y88888P"   "Y8888P"
"#;

struct LogIfImpl;

#[crate_interface::impl_interface]
impl axlog::LogIf for LogIfImpl {
    fn console_write_str(s: &str) {
        axhal::console::write_bytes(s.as_bytes());
    }

    fn current_time() -> core::time::Duration {
        axhal::time::current_time()
    }

    fn current_cpu_id() -> Option<usize> {
        #[cfg(feature = "smp")]
        if is_init_ok() {
            Some(axhal::cpu::this_cpu_id())
        } else {
            None
        }
        #[cfg(not(feature = "smp"))]
        Some(0)
    }

    fn current_task_id() -> Option<u64> {
        None
    }
}

/// The main entry point for monolithic kernel startup.
#[cfg_attr(not(test), no_mangle)]
pub fn rust_main(cpu_id: usize, dtb: usize) -> ! {
    ax_println!("{}", LOGO);
    ax_println!(
        "\
        arch = {}\n\
        platform = {}\n\
        target = {}\n\
        smp = {}\n\
        build_mode = {}\n\
        log_level = {}\n\
        ",
        option_env!("AX_ARCH").unwrap_or(""),
        option_env!("AX_PLATFORM").unwrap_or(""),
        option_env!("AX_TARGET").unwrap_or(""),
        option_env!("AX_SMP").unwrap_or(""),
        option_env!("AX_MODE").unwrap_or(""),
        option_env!("AX_LOG").unwrap_or(""),
    );

    axlog::init();
    axlog::set_max_level(option_env!("AX_LOG").unwrap_or("")); // no effect if set `log-level-*` features
    info!("Logging is enabled.");
    info!(
        "MacroKernel is starting: Primary CPU {} started, dtb = {:#x}.",
        cpu_id, dtb
    );

    info!("Found physcial memory regions:");
    for r in axhal::mem::memory_regions() {
        info!(
            "  [{:x?}, {:x?}) {} ({:?})",
            r.paddr,
            r.paddr + r.size,
            r.name,
            r.flags
        );
    }

    info!("Initialize kernel page table...");
    remap_kernel_memory().expect("remap kernel memoy failed");

    info!("Initialize platform devices...");
    axhal::platform_init();

    task::init();
    run_queue::init();

    {
        let all_devices = axdriver::init_drivers();
        let main_fs = axmount::init_filesystems(all_devices.block);
        let root_dir = axmount::init_rootfs(main_fs);
        task::current().fs.lock().init(root_dir);
    }

    #[cfg(feature = "smp")]
    self::mp::start_secondary_cpus(cpu_id);

    info!("Initialize interrupt handlers...");
    init_interrupt();

    axsyscall::init();

    info!("Primary CPU {} init OK.", cpu_id);
    INITED_CPUS.fetch_add(1, Ordering::Relaxed);

    while !is_init_ok() {
        core::hint::spin_loop();
    }

    start_kernel(dtb).expect("Fatal error!");

    panic!("Never reach here!");
}

fn start_kernel(dtb: usize) -> LinuxResult {
    let dtb_info = setup_arch(dtb)?;
    rest_init(dtb_info);
    Ok(())
}

fn setup_arch(dtb: usize) -> LinuxResult<DtbInfo> {
    parse_dtb(dtb)
}

fn parse_dtb(dtb_pa: usize) -> LinuxResult<DtbInfo> {
    let mut dtb_info = DtbInfo::new();
    let mut cb = |name: String, _addr_cells: usize, _size_cells: usize, props: Vec<(String, Vec<u8>)>| {
        if name == "chosen" {
            for prop in props {
                match prop.0.as_str() {
                    "bootargs" => {
                        if let Ok(cmd) = core::str::from_utf8(&prop.1) {
                            parse_cmdline(cmd, &mut dtb_info);
                        }
                    },
                    _ => (),
                }
            }
        }
    };

    let dtb_va = phys_to_virt(dtb_pa.into());
    let dt = axdtb::DeviceTree::init(dtb_va.into()).unwrap();
    dt.parse(dt.off_struct, 0, 0, &mut cb).unwrap();

    Ok(dtb_info)
}

fn parse_cmdline(cmd: &str, dtb_info: &mut DtbInfo) {
    let cmd = cmd.trim_end_matches(char::from(0));
    if cmd.len() > 0 {
        assert!(cmd.starts_with("init="));
        let cmd = cmd.strip_prefix("init=").unwrap();
        dtb_info.set_init_cmd(cmd);
    }
}

fn remap_kernel_memory() -> Result<(), axhal::paging::PagingError> {
    use axhal::paging::PageTable;
    use axhal::paging::{reuse_page_table_root, setup_page_table_root};

    if this_cpu_is_bsp() {
        let mut kernel_page_table = PageTable::try_new()?;
        for r in memory_regions() {
            kernel_page_table.map_region(
                phys_to_virt(r.paddr),
                r.paddr,
                r.size,
                r.flags.into(),
                true,
            )?;
        }
        setup_page_table_root(kernel_page_table);
    } else {
        reuse_page_table_root();
    }

    Ok(())
}

// Todo: Consider to move it to standalone component 'cpu'
fn this_cpu_is_bsp() -> bool {
    let _ = NoPreempt::new();
    axhal::cpu::_this_cpu_is_bsp()
}

fn init_interrupt() {
    use axhal::time::TIMER_IRQ_NUM;

    // Setup timer interrupt handler
    const PERIODIC_INTERVAL_NANOS: u64 =
        axhal::time::NANOS_PER_SEC / axconfig::TICKS_PER_SEC as u64;

    #[percpu::def_percpu]
    static NEXT_DEADLINE: u64 = 0;

    fn update_timer() {
        let now_ns = axhal::time::current_time_nanos();
        // Safety: we have disabled preemption in IRQ handler.
        let mut deadline = unsafe { NEXT_DEADLINE.read_current_raw() };
        if now_ns >= deadline {
            deadline = now_ns + PERIODIC_INTERVAL_NANOS;
        }
        unsafe { NEXT_DEADLINE.write_current_raw(deadline + PERIODIC_INTERVAL_NANOS) };
        axhal::time::set_oneshot_timer(deadline);
    }

    axtrap::irq::register_handler(TIMER_IRQ_NUM, || {
        update_timer();
        run_queue::on_timer_tick();
    });

    // Enable IRQs before starting app
    axhal::arch::enable_irqs();
}

fn rest_init(dtb_info: DtbInfo) {
    error!("rest_init ...");
    let pid = user_mode_thread(
        move || {
            kernel_init(dtb_info);
        },
        CloneFlags::CLONE_FS,
    );
    assert_eq!(pid, 1);

    /*
     * The boot idle thread must execute schedule()
     * at least once to get things moving:
     */
    schedule_preempt_disabled();
    /* Call into cpu_idle with preempt disabled */
    cpu_startup_entry(/* CPUHP_ONLINE */);
}

fn schedule_preempt_disabled() {
    let task = task::current();
    let rq = run_queue::task_rq(&task.sched_info);
    rq.lock().resched(false);
    unimplemented!("schedule_preempt_disabled()");
}

fn cpu_startup_entry() {
    unimplemented!("do idle()");
}

/// Prepare for entering first user app.
fn kernel_init(dtb_info: DtbInfo) {
    /*
     * We try each of these until one succeeds.
     *
     * The Bourne shell can be used instead of init if we are
     * trying to recover a really broken machine.
     */
    if let Some(cmd) = dtb_info.get_init_cmd() {
        run_init_process(cmd)
            .unwrap_or_else(|_| panic!("Requested init {} failed.", cmd));
        return;
    }

    try_to_run_init_process("/sbin/init").expect("No working init found.");
}

fn try_to_run_init_process(init_filename: &str) -> LinuxResult {
    run_init_process(init_filename).inspect_err(|e| {
        if e != &LinuxError::ENOENT {
            error!(
                "Starting init: {} exists but couldn't execute it (error {})",
                init_filename, e
            );
        }
    })
}

fn run_init_process(init_filename: &str) -> LinuxResult {
    error!("run_init_process...");
    exec::kernel_execve(init_filename)
}
