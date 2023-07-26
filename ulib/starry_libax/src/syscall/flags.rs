use axconfig::TICKS_PER_SEC;
use axhal::{
    paging::MappingFlags,
    time::{current_time_nanos, MICROS_PER_SEC, NANOS_PER_MICROS, NANOS_PER_SEC},
};
use bitflags::*;
use log::error;
pub const NSEC_PER_SEC: usize = 1_000_000_000;
bitflags! {
    /// 指定 sys_wait4 的选项
    pub struct WaitFlags: u32 {
        /// 不挂起当前进程，直接返回
        const WNOHANG = 1 << 0;
        /// 报告已执行结束的用户进程的状态
        const WIMTRACED = 1 << 1;
        /// 报告还未结束的用户进程的状态
        const WCONTINUED = 1 << 3;
    }
}
/// sys_times 中指定的结构体类型
#[repr(C)]
pub struct TMS {
    /// 进程用户态执行时间，单位为us
    pub tms_utime: usize,
    /// 进程内核态执行时间，单位为us
    pub tms_stime: usize,
    /// 子进程用户态执行时间和，单位为us
    pub tms_cutime: usize,
    /// 子进程内核态执行时间和，单位为us
    pub tms_cstime: usize,
}

/// sys_gettimeofday 中指定的类型
#[repr(C)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

impl TimeVal {
    pub fn to_nanos(&self) -> usize {
        self.sec * NANOS_PER_SEC as usize + self.usec * NANOS_PER_MICROS as usize
    }
    pub fn from_micro(micro: usize) -> Self {
        TimeVal {
            sec: micro / (MICROS_PER_SEC as usize),
            usec: micro % (MICROS_PER_SEC as usize),
        }
    }
}

/// sys_gettimer / sys_settimer 指定的类型，用户输入输出计时器
pub struct ITimerVal {
    pub it_interval: TimeVal,
    pub it_value: TimeVal,
}

// sys_nanosleep指定的结构体类型
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeSecs {
    pub tv_sec: usize,
    pub tv_nsec: usize,
}
/// 当 nsec 为这个特殊值时，指示修改时间为现在
pub const UTIME_NOW: usize = 0x3fffffff;
/// 当 nsec 为这个特殊值时，指示不修改时间
pub const UTIME_OMIT: usize = 0x3ffffffe;
impl TimeSecs {
    /// 从秒数和纳秒数构造一个 TimeSecs
    pub fn now() -> Self {
        let nano = current_time_nanos() as usize;
        let tv_sec = nano / NSEC_PER_SEC;
        let tv_nsec = nano - tv_sec * NSEC_PER_SEC;
        TimeSecs { tv_sec, tv_nsec }
    }

    pub fn to_nano(&self) -> usize {
        self.tv_sec * NSEC_PER_SEC + self.tv_nsec
    }

    pub fn get_ticks(&self) -> usize {
        self.tv_sec * TICKS_PER_SEC + self.tv_nsec * TICKS_PER_SEC / (NANOS_PER_SEC as usize)
    }

    pub fn set_as_utime(&mut self, other: &TimeSecs) {
        match other.tv_nsec {
            UTIME_NOW => {
                *self = TimeSecs::now();
            } // 设为当前时间
            UTIME_OMIT => {} // 忽略
            _ => {
                *self = *other;
            } // 设为指定时间
        }
    }
}

bitflags! {
    #[derive(Debug)]
    /// 指定 mmap 的选项
    pub struct MMAPPROT: u32 {
        /// 区域内容可读取
        const PROT_READ = 1 << 0;
        /// 区域内容可修改
        const PROT_WRITE = 1 << 1;
        /// 区域内容可执行
        const PROT_EXEC = 1 << 2;
    }
}

impl Into<MappingFlags> for MMAPPROT {
    fn into(self) -> MappingFlags {
        let mut flags = MappingFlags::USER;
        if self.contains(MMAPPROT::PROT_READ) {
            flags |= MappingFlags::READ;
        }
        if self.contains(MMAPPROT::PROT_WRITE) {
            flags |= MappingFlags::WRITE;
        }
        if self.contains(MMAPPROT::PROT_EXEC) {
            flags |= MappingFlags::EXECUTE;
        }
        flags
    }
}

bitflags! {
    #[derive(Debug)]
    pub struct MMAPFlags: u32 {
        /// 对这段内存的修改是共享的
        const MAP_SHARED = 1 << 0;
        /// 对这段内存的修改是私有的
        const MAP_PRIVATE = 1 << 1;
        // 以上两种只能选其一

        /// 取消原来这段位置的映射，即一定要映射到指定位置
        const MAP_FIXED = 1 << 4;
        /// 不映射到实际文件
        const MAP_ANONYMOUS = 1 << 5;
        /// 映射时不保留空间，即可能在实际使用mmp出来的内存时内存溢出
        const MAP_NORESERVE = 1 << 14;
    }
}

/// sys_uname 中指定的结构体类型
#[repr(C)]
pub struct UtsName {
    /// 系统名称
    pub sysname: [u8; 65],
    /// 网络上的主机名称
    pub nodename: [u8; 65],
    /// 发行编号
    pub release: [u8; 65],
    /// 版本
    pub version: [u8; 65],
    /// 硬件类型
    pub machine: [u8; 65],
    /// 域名
    pub domainname: [u8; 65],
}

impl UtsName {
    /// 默认的 UtsName，并没有统一标准
    pub fn default() -> Self {
        Self {
            sysname: Self::from_str("YoimiyaOS"),
            nodename: Self::from_str("YoimiyaOS - machine[0]"),
            release: Self::from_str("114"),
            version: Self::from_str("1.0"),
            machine: Self::from_str("RISC-V 64 on SIFIVE FU740"),
            domainname: Self::from_str("https://github.com/Azure-stars/arceos"),
        }
    }

    fn from_str(info: &str) -> [u8; 65] {
        let mut data: [u8; 65] = [0; 65];
        data[..info.len()].copy_from_slice(info.as_bytes());
        data
    }
}

pub(crate) unsafe fn get_str_len(start: *const u8) -> usize {
    let mut ptr = start as usize;
    while *(ptr as *const u8) != 0 {
        ptr += 1;
    }
    ptr - start as usize
}

pub(crate) unsafe fn raw_ptr_to_ref_str(start: *const u8) -> &'static str {
    let len = get_str_len(start);
    // 因为这里直接用用户空间提供的虚拟地址来访问，所以一定能连续访问到字符串，不需要考虑物理地址是否连续
    let slice = core::slice::from_raw_parts(start, len);
    if let Ok(s) = core::str::from_utf8(slice) {
        s
    } else {
        error!("not utf8 slice");
        for c in slice {
            error!("{c} ");
        }
        error!("");
        &"p"
    }
}

pub const SIGSET_SIZE_IN_BYTE: usize = 8;

pub enum SigMaskFlag {
    SigBlock = 0,
    SigUnblock = 1,
    SigSetmask = 2,
}

impl SigMaskFlag {
    pub fn from(value: usize) -> Self {
        match value {
            0 => SigMaskFlag::SigBlock,
            1 => SigMaskFlag::SigUnblock,
            2 => SigMaskFlag::SigSetmask,
            _ => panic!("SIG_MASK_FLAG::from: invalid value"),
        }
    }
}

/// sys_prlimit64 使用的数组
#[repr(C)]
pub struct RLimit {
    /// 软上限
    pub rlim_cur: u64,
    /// 硬上限
    pub rlim_max: u64,
}
// sys_prlimit64 使用的选项
/// 用户栈大小
pub const RLIMIT_STACK: i32 = 3;
/// 可以打开的 fd 数
pub const RLIMIT_NOFILE: i32 = 7;
/// 用户地址空间的最大大小
pub const RLIMIT_AS: i32 = 9;

/// robust list
#[repr(C)]
pub struct RobustList {
    pub head: usize,
    pub off: usize,
    pub pending: usize,
}

/// readv/writev使用的结构体
#[repr(C)]
pub struct IoVec {
    pub base: *mut u8,
    pub len: usize,
}
/// 对 futex 的操作
pub enum FutexFlags {
    /// 检查用户地址 uaddr 处的值。如果不是要求的值则等待 wake
    WAIT,
    /// 唤醒最多 val 个在等待 uaddr 位置的线程。
    WAKE,
    REQUEUE,
    UNSUPPORTED,
}

impl FutexFlags {
    pub fn new(val: i32) -> Self {
        match val & 0x7f {
            0 => FutexFlags::WAIT,
            1 => FutexFlags::WAKE,
            3 => FutexFlags::REQUEUE,
            _ => FutexFlags::UNSUPPORTED,
        }
    }
}

numeric_enum_macro::numeric_enum! {
    #[repr(usize)]
    #[allow(non_camel_case_types)]
    #[derive(Debug)]
    /// sys_fcntl64 使用的选项
    pub enum Fcntl64Cmd {
        /// 复制这个 fd，相当于 sys_dup
        F_DUPFD = 0,
        /// 获取 cloexec 信息，即 exec 成功时是否删除该 fd
        F_GETFD = 1,
        /// 设置 cloexec 信息，即 exec 成功时删除该 fd
        F_SETFD = 2,
        /// 获取 flags 信息
        F_GETFL = 3,
        /// 设置 flags 信息
        F_SETFL = 4,
        /// 复制 fd，然后设置 cloexec 信息，即 exec 成功时删除该 fd
        F_DUPFD_CLOEXEC = 1030,
    }
}

/// syscall_info 用到的 结构体
#[repr(C)]
#[derive(Debug)]
pub struct SysInfo {
    /// 启动时间(以秒计)
    pub uptime: isize,
    /// 1 / 5 / 15 分钟平均负载
    pub loads: [usize; 3],
    /// 内存总量，单位为 mem_unit Byte(见下)
    pub totalram: usize,
    /// 当前可用内存，单位为 mem_unit Byte(见下)
    pub freeram: usize,
    /// 共享内存大小，单位为 mem_unit Byte(见下)
    pub sharedram: usize,
    /// 用于缓存的内存大小，单位为 mem_unit Byte(见下)
    pub bufferram: usize,
    /// swap空间大小，即主存上用于替换内存中非活跃部分的空间大小，单位为 mem_unit Byte(见下)
    pub totalswap: usize,
    /// 可用的swap空间大小，单位为 mem_unit Byte(见下)
    pub freeswap: usize,
    /// 当前进程数，单位为 mem_unit Byte(见下)
    pub procs: u16,
    /// 高地址段的内存大小，单位为 mem_unit Byte(见下)
    pub totalhigh: usize,
    /// 可用的高地址段的内存大小，单位为 mem_unit Byte(见下)
    pub freehigh: usize,
    /// 指定 sys_info 的结构中用到的内存值的单位。
    /// 如 mem_unit = 1024, totalram = 100, 则指示总内存为 100K
    pub mem_unit: u32,
}

// sys_getrusage 用到的选项
#[allow(non_camel_case_types)]
pub enum RusageFlags {
    /// 获取当前进程的资源统计
    RUSAGE_SELF = 0,
    /// 获取当前进程的所有 **已结束并等待资源回收的** 子进程资源统计
    RUSAGE_CHILDREN = -1,
    /// 获取当前线程的资源统计
    RUSAGE_THREAD = 1,
}

impl RusageFlags {
    pub fn from(val: i32) -> Option<Self> {
        match val {
            0 => Some(RusageFlags::RUSAGE_SELF),
            -1 => Some(RusageFlags::RUSAGE_CHILDREN),
            1 => Some(RusageFlags::RUSAGE_THREAD),
            _ => None,
        }
    }
}