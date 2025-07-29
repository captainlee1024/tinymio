use crate::{Events, Interests, Token};
use std::io::{IoSliceMut, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{io, net, ptr};

pub type Source = std::os::unix::io::RawFd;

pub struct Registrator {
    kq: Source,
    is_poll_dead: Arc<AtomicBool>,
}

impl Registrator {
    pub fn register(
        &self,
        stream: &TcpStream,
        token: usize,
        interests: Interests,
    ) -> io::Result<()> {
        if self.is_poll_dead.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Poll instance closed.",
            ));
        }

        let fd = stream.as_raw_fd();
        if interests.is_readable() {
            let event = ffi::Event::new_read_event(fd, token as u64);
            let event = [event];
            kevent(self.kq, &event, &mut [], 0, None)?;
        };

        if interests.is_writable() {
            unimplemented!();
        }

        Ok(())
    }

    pub fn close_loop(&self) -> io::Result<()> {
        if self
            .is_poll_dead
            .compare_and_swap(false, true, Ordering::SeqCst)
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Poll instance closed.",
            ));
        }

        let event = ffi::Event::new_wakeup_event();
        let event = [event];
        kevent(self.kq, &event, &mut [], 0, None)?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct Selector {
    kq: Source,
}

impl Selector {
    pub fn new() -> io::Result<Self> {
        Ok(Selector { kq: kqueue()? })
    }

    pub fn select(&self, events: &mut Events, timeout_ms: Option<i32>) -> io::Result<()> {
        let n_events = events.capacity() as i32;
        events.clear();
        kevent(self.kq, &[], events, n_events, timeout_ms).map(|n_events| {
            unsafe { events.set_len(n_events as usize) };
        })
    }

    pub fn registrator(&self, is_poll_dead: Arc<AtomicBool>) -> Registrator {
        Registrator {
            kq: self.kq,
            is_poll_dead,
        }
    }
}

impl Drop for Selector {
    fn drop(&mut self) {
        match close(self.kq) {
            Ok(..) => (),
            Err(e) => {
                if !std::thread::panicking() {
                    panic!("{e}");
                }
            }
        }
    }
}

pub type Event = ffi::Event;
impl Event {
    pub fn id(&self) -> Token {
        self.udata as usize
    }
}

pub struct TcpStream {
    inner: net::TcpStream,
}

impl TcpStream {
    pub fn connect(addr: impl net::ToSocketAddrs) -> io::Result<Self> {
        let stream = net::TcpStream::connect(addr)?;
        stream.set_nonblocking(true)?;
        Ok(TcpStream { inner: stream })
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.set_nonblocking(false)?;
        (&self.inner).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        (&self.inner).read_vectored(bufs)
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

mod ffi {
    use super::*;
    use crate::Token;

    pub const EVFILT_READ: i16 = -1;
    pub const EVFILT_TIMER: i16 = -7;
    pub const EV_ADD: u16 = 0x1;
    pub const EV_ENABLE: u16 = 0x4;
    pub const EV_ONESHOT: u16 = 0x10;
    pub const EV_CLEAR: u16 = 0x20;

    // To be able to pass in a timeout to `Kqueue`we need to use
    // a timespec struct to pass in the information
    #[derive(Debug)]
    #[repr(C)]
    pub(super) struct Timespec {
        /// Seconds
        tv_sec: isize,
        /// Nanoseconds     
        v_nsec: usize,
    }

    impl Timespec {
        /// Convenience function so that we can easily create a `timespec` struct
        /// from milliseconds. We won't support granularity smaller than ms
        /// in our library even though we could on macos.
        pub fn from_millis(milliseconds: i32) -> Self {
            let seconds = milliseconds / 1000;
            let nanoseconds = (milliseconds % 1000) * 1000 * 1000;
            Timespec {
                tv_sec: seconds as isize,
                v_nsec: nanoseconds as usize,
            }
        }
    }

    pub type Event = Kevent;
    impl Event {
        pub fn new_read_event(fd: RawFd, id: u64) -> Self {
            Event {
                ident: fd as u64,
                filter: EVFILT_READ,
                flags: EV_ADD | EV_ENABLE | EV_ONESHOT,
                fflags: 0,
                data: 0,
                udata: id,
            }
        }

        pub fn new_wakeup_event() -> Self {
            Event {
                ident: 0,
                filter: EVFILT_TIMER,
                flags: EV_ADD | EV_ENABLE | EV_CLEAR,
                fflags: 0,
                // data is where our timeout will be set but we want to timeout immideately
                data: 0,
                udata: 0, // TODO: see if windows needs u32...
            }
        }

        pub fn zero() -> Self {
            Event {
                ident: 0,
                filter: 0,
                flags: 0,
                fflags: 0,
                data: 0,
                udata: 0,
            }
        }
    }

    // Kevent结构体 是kqueue的瑞士军刀，它有两个关键作用：
    //  1. 作为输入，描述你想要的 change，比如，帮我添加一个对socket a的读事件监听
    //  2. 作为输出，描述一个已经发生的 event, 比如 socket a 现在可读了
    // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L497
    // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L207
    #[derive(Debug, Clone, Default)]
    #[repr(C)]
    pub struct Kevent {
        // 比如我们监听socket是否可以读时，ident就是我们要监听的socket 的 fd
        pub ident: u64,
        // 监听socket 可读时设置为 EVFILT_READ
        pub filter: i16,
        // 类似于epoll的op, 比如我们添加一个一次性的监听就是 EV_ADD | EV_ONESHOT
        pub flags: u16,
        pub fflags: u32,
        pub data: i64,
        // 设置我们的Token, 用于我们自己标识事件源
        pub udata: u64,
    }

    impl Kevent {
        pub fn token(&self) -> Option<Token> {
            // we have no realiable way of checking if this value is initialized or not but need
            // an option to be compatible with windows.
            Some(self.udata as usize)
        }
    }

    #[link(name = "c")]
    extern "C" {
        /// Returns: positive: file descriptor, negative: error
        ///     // Kqueue()系统调用，这个调用创建一个新的内核事件队列实例，并返回一个指向它的文件描述符
        //     // 这个kqueue fd 就是之后所有操作的句柄
        pub(super) fn kqueue() -> i32;
        /// Returns: nothing, all non zero return values is an error
        /// If the time limit expires, then kevent() returns 0
        /// Kevent 系统调用，这是与kqueue交互的唯一函数
        ///     1. 它接受一个输入： change list, 即一个kevent vec
        ///     2. 它接收一个空输出：event list, 作为输出缓冲区
        /// 当输入 change list 不为空时，内核会应用这些变更
        /// 然后它会检查是否有已出发的event, 如果有就填充到event list缓冲区并返回，如果没有当前事件它会阻塞，或者根据传入的超时限制返回超时
        ///
        /// Filters, kqueue使用过滤器来定义事件类型，比如 EVFILT_READ 可读， EVFILT_WRITE 可写, EVFILT_TIMER 定时器
        /// 这是 kqueue与epoll的一个关键区别
        pub(super) fn kevent(
            kq: i32,
            changelist: *const Kevent,
            nchanges: i32,
            eventlist: *mut Kevent,
            nevents: i32,
            timeout: *const Timespec,
        ) -> i32;

        pub fn close(d: i32) -> i32;
    }
}

// 使用系统调用创建一个kqueue句柄
pub fn kqueue() -> io::Result<i32> {
    let fd = unsafe { ffi::kqueue() };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

// 两次调用：
//  1. 将changelist提交给内核
//  2. 第二次调用就是创建一个空的event数组，阻塞直到我们提交的changelist 中的event发生，
//  现在操作系统暂停我们的线程进行上下文切换并处理其他事情或者只保留电源
pub fn kevent(
    kq: RawFd,
    cl: &[ffi::Kevent],
    el: &mut [ffi::Kevent],
    n_events: i32,
    timeout_ms: Option<i32>,
) -> io::Result<usize> {
    let res = unsafe {
        let kq = kq as i32;
        let cl_len = cl.len() as i32;

        let timeout = timeout_ms.map(ffi::Timespec::from_millis);

        let timeout: *const ffi::Timespec = match &timeout {
            Some(n) => n,
            None => ptr::null(),
        };

        ffi::kevent(kq, cl.as_ptr(), cl_len, el.as_mut_ptr(), n_events, timeout)
    };
    if res < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(res as usize)
}

pub fn close(fd: RawFd) -> io::Result<()> {
    let res = unsafe { ffi::close(fd) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
