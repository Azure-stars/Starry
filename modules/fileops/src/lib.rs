#![cfg_attr(not(test), no_std)]

#[macro_use]
extern crate log;

extern crate alloc;
use alloc::sync::Arc;
use alloc::string::String;

use axerrno::LinuxError;
use axfile::fops::File;
use axfile::fops::OpenOptions;
use axfile::api::create_dir;
use mutex::Mutex;

// Special value used to indicate openat should use
// the current working directory.
pub const AT_FDCWD: usize = -100isize as usize;

const O_CREAT: usize = 0o100;

pub fn openat(dfd: usize, filename: &str, flags: usize, mode: usize) -> usize {
    info!("openat '{}' at dfd {:#X} flags {:#X} mode {:#X}",
        filename, dfd, flags, mode);

    let mut opts = OpenOptions::new();
    opts.read(true);
    if (flags & O_CREAT) != 0 {
        opts.write(true);
        opts.create(true);
        opts.truncate(true);
    }

    let current = task::current();
    let fs = current.fs.lock();

    let path = handle_path(dfd, filename);
    info!("openat path {}", path);
    let file = match File::open(&path, &opts, &fs) {
        Ok(f) => f,
        Err(e) => {
            error!("openat path {} failed.", path);
            return (-LinuxError::from(e).code()) as usize;
        },
    };
    let fd = current.filetable.lock().insert(Arc::new(Mutex::new(file)));
    info!("openat {} return fd {}", path, fd);
    fd
}

fn handle_path(dfd: usize, filename: &str) -> String {
    if dfd == AT_FDCWD {
        let cwd = _getcwd();
        if cwd == "/" {
            assert!(filename.starts_with("/"));
        } else {
            return cwd + filename;
        }
    }
    String::from(filename)
}

pub fn read(fd: usize, ubuf: &mut [u8]) -> usize {
    let count = ubuf.len();
    let current = task::current();
    let file = current.filetable.lock().get_file(fd).unwrap();
    let mut pos = 0;
    assert!(count < 1024);
    let mut kbuf: [u8; 1024] = [0; 1024];
    while pos < count {
        let ret = file.lock().read(&mut kbuf[pos..]).unwrap();
        if ret == 0 {
            break;
        }
        pos += ret;
    }

    info!("linux_syscall_read: fd {}, count {}, ret {}", fd, count, pos);

    axhal::arch::enable_sum();
    ubuf.copy_from_slice(&kbuf[..count]);
    axhal::arch::disable_sum();
    pos
}

pub fn write(fd: usize, ubuf: &[u8]) -> usize {
    if fd == 1 || fd == 2 {
        return write_to_stdio(ubuf);
    }

    let count = ubuf.len();
    let current = task::current();
    let file = current.filetable.lock().get_file(fd).unwrap();
    let mut pos = 0;
    assert!(count < 1024);
    axhal::arch::enable_sum();
    while pos < count {
        let ret = file.lock().write(&ubuf[pos..]).unwrap();
        if ret == 0 {
            break;
        }
        pos += ret;
    }
    axhal::arch::disable_sum();
    info!("write: fd {}, count {}, ret {}", fd, count, pos);
    pos
}

fn write_to_stdio(ubuf: &[u8]) -> usize {
    axhal::arch::enable_sum();
    axhal::console::write_bytes(ubuf);
    axhal::arch::disable_sum();
    ubuf.len()
}

#[derive(Debug)]
#[repr(C)]
pub struct iovec {
    iov_base: usize,
    iov_len: usize,
}

pub fn writev(fd: usize, iov_array: &[iovec]) -> usize {
    assert!(fd == 1 || fd == 2);
    axhal::arch::enable_sum();
    for iov in iov_array {
        debug!("iov: {:#X} {:#X}", iov.iov_base, iov.iov_len);
        let bytes = unsafe { core::slice::from_raw_parts(iov.iov_base as *const _, iov.iov_len) };
        let s = String::from_utf8(bytes.into());
        error!("{}", s.unwrap());
    }
    axhal::arch::disable_sum();
    iov_array.len()
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct KernelStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub _pad0: u64,
    pub st_size: u64,
    pub st_blksize: u32,
    pub _pad1: u32,
    pub st_blocks: u64,
    pub st_atime_sec: isize,
    pub st_atime_nsec: isize,
    pub st_mtime_sec: isize,
    pub st_mtime_nsec: isize,
    pub st_ctime_sec: isize,
    pub st_ctime_nsec: isize,
}

pub fn fstatat(dirfd: usize, _path: &str, statbuf_ptr: usize, _flags: usize) -> usize {
    if dirfd == 1 {
        // Todo: Handle stdin(0), stdout(1) and stderr(2)
        let statbuf = statbuf_ptr as *mut KernelStat;
        axhal::arch::enable_sum();
        unsafe {
            *statbuf = KernelStat {
                st_mode: 0x2180,
                st_nlink: 1,
                st_blksize: 0x1000,
                st_ino: 0x2a,
                st_dev: 2,
                st_rdev: 0x500001,
                st_size: 0,
                st_blocks: 0,
                //st_uid: 1000,
                //st_gid: 1000,
                ..Default::default()
            };
        }
        axhal::arch::disable_sum();
        return 0;
    }

    assert!(dirfd > 2);

    let current = task::current();
    let filetable = current.filetable.lock();
    let file = match filetable.get_file(dirfd) {
        Some(f) => f,
        None => {
            return (-2isize) as usize;
        },
    };
    let metadata = file.lock().get_attr().unwrap();
    let ty = metadata.file_type() as u8;
    let perm = metadata.perm().bits() as u32;
    let st_mode = ((ty as u32) << 12) | perm;
    let st_size = metadata.size();
    error!("st_size: {}", st_size);

    let statbuf = statbuf_ptr as *mut KernelStat;
    axhal::arch::enable_sum();
    unsafe {
        *statbuf = KernelStat {
            st_ino: 1,
            st_nlink: 1,
            st_mode,
            st_uid: 1000,
            st_gid: 1000,
            st_size: st_size,
            st_blocks: metadata.blocks() as _,
            st_blksize: 512,
            ..Default::default()
        };
    }
    axhal::arch::disable_sum();
    0
}

// IOCTL
const TCGETS: usize = 0x5401;

const NCCS: usize = 19;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
struct Termios {
    c_iflag: u32,   /* input mode flags */
    c_oflag: u32,   /* output mode flags */
    c_cflag: u32,   /* control mode flags */
    c_lflag: u32,   /* local mode flags */
    c_line:  u8,    /* line discipline */
    c_cc:    [u8; NCCS], /* control characters */
}

pub fn ioctl(fd: usize, request: usize, udata: usize) -> usize {
    info!("linux_syscall_ioctl fd {}, request {:#X}, udata {:#X}",
        fd, request, udata);

    assert_eq!(fd, 1);
    assert_eq!(request, TCGETS);

    let cc: [u8; NCCS] = [
        0x3, 0x1c, 0x7f, 0x15, 0x4, 0x0, 0x1, 0x0,
        0x11, 0x13, 0x1a, 0x0, 0x12, 0xf, 0x17, 0x16,
        0x0, 0x0, 0x0,
    ];

    let ubuf = udata as *mut Termios;
    axhal::arch::enable_sum();
    unsafe {
        *ubuf = Termios {
            c_iflag: 0x500,
            c_oflag: 0x5,
            c_cflag: 0xcbd,
            c_lflag: 0x8a3b,
            c_line: 0,
            c_cc: cc,
        };
    }
    axhal::arch::disable_sum();
    0
}

pub fn mkdirat(dfd: usize, pathname: &str, mode: usize) -> usize {
    info!("mkdirat: dfd {:#X}, pathname {}, mode {:#X}", dfd, pathname, mode);
    assert_eq!(dfd, AT_FDCWD);

    let current = task::current();
    let fs = current.fs.lock();
    match create_dir(pathname, &fs) {
        Ok(()) => 0,
        Err(e) => {
            (-LinuxError::from(e).code()) as usize
        },
    }
}

pub fn getcwd(buf: &mut [u8]) -> usize {
    let cwd = _getcwd();
    info!("getcwd {}", cwd);
    let bytes = cwd.as_bytes();
    let count = bytes.len();
    axhal::arch::enable_sum();
    buf[0..count].copy_from_slice(bytes);
    buf[count] = 0u8;
    axhal::arch::disable_sum();
    count + 1
}

fn _getcwd() -> String {
    let current = task::current();
    let fs = current.fs.lock();
    fs.current_dir().expect("bad cwd")
}

pub fn chdir(path: &str) -> usize {
    let current = task::current();
    let mut fs = current.fs.lock();
    match fs.set_current_dir(path) {
        Ok(()) => 0,
        Err(e) => {
            (-LinuxError::from(e).code()) as usize
        },
    }
}
