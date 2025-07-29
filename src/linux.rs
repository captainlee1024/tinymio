use crate::{Events, Interests, Token};
use std::io::{self, IoSliceMut, Read, Write};
use std::net;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

pub struct Registrator {
    epoll_fd: RawFd,
    is_poll_dead: Arc<AtomicBool>,
}

impl Registrator {
    // 封装ffi epoll_crate 提供rust的事件注册功能
    pub fn register(
        &self,
        stream: &TcpStream,
        token: usize,
        interests: Interests,
    ) -> io::Result<()> {
        // 检查是否关闭
        if self.is_poll_dead.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Poll instance closed",
            ))
        }

        // 获取stream socket的fd
        let fd = stream.as_raw_fd();
        // 注册socket可读事件
        if interests.is_readable() {
            // 然后注册对接收此套接字上 `Read` 事件通知的兴趣。`Event` 结构体用于指定要注册兴趣的事件以及其他使用标志的配置。
            //
            // `EPOLLIN` 表示对 `Read` 事件的兴趣。
            // `EPOLLONESHOT` 表示在第一个事件之后从队列中移除所有兴趣。如果不这样做，我们需要在套接字处理完毕后手动 `deregister` 我们的兴趣。
            //
            // `epoll_data` 是用户提供的数据，因此我们可以在其中放置一个指针或整数值来标识事件。我们仅使用“i”即循环计数来识别事件。
            let mut event = ffi::Event::new(ffi::EPOLLIN | ffi::EPOLLONESHOT, token);
            epoll_ctl(self.epoll_fd, ffi::EPOLL_CTL_ADD, fd, &mut event)?;
        };

        if interests.is_writable() {
            unimplemented!();
        }

        Ok(())
    }


    // 将is_poll_dead设置为true之后，这里发送最后一个事件，关闭队列
    pub fn close_loop(&self) -> io::Result<()> {
        if self.is_poll_dead.compare_and_swap(false, true, Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Poll instance closed",
            ));
        }

        // TODO: 优化这里，能达到目的，写法不好
        let wake_fd = eventfd(1, 0)?;
        let mut event = ffi::Event::new(ffi::EPOLLIN, 0);
        epoll_ctl(self.epoll_fd, ffi::EPOLL_CTL_ADD, wake_fd, &mut event)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct Selector {
    epoll_fd: RawFd,
}

impl Selector {
    pub fn new() -> io::Result<Self> {
        Ok(Selector {
            epoll_fd: epoll_create()?,
        })
    }

    pub fn select(&self, events: &mut Events, timeout_ms: Option<i32>) -> io::Result<()> {
        events.clear();
        let timeout = timeout_ms.unwrap_or(-1);
        epoll_wait(self.epoll_fd, events, 1024, timeout).map(|n_events| {
            unsafe { events.set_len(n_events as usize) };
        })
    }

    pub fn registrator(&self, is_poll_dead: Arc<AtomicBool>) -> Registrator {
        Registrator{
            epoll_fd: self.epoll_fd,
            is_poll_dead,
        }
    }
}

impl Drop for Selector {
    fn drop(&mut self) {
        match close(self.epoll_fd) {
            Ok(..) => (),
            Err(e) => {
                if !std::thread::panicking() {
                    panic!("{}", e);
                }
            }
        }
    }
}

pub type Event = ffi::Event;
impl Event {
    pub fn id(&self) -> Token{ self.data()}
}

pub struct TcpStream {
    inner: net::TcpStream,
}

impl TcpStream {
    pub fn connect(addr: impl net::ToSocketAddrs) -> io::Result<Self> {
        let stream = net::TcpStream::connect(addr)?;
        stream.set_nonblocking(true)?;

        Ok(TcpStream{ inner: stream})
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.set_nonblocking(false)?;
        (&self.inner).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> io::Result<usize> {
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

    pub const EPOLL_CTL_ADD: i32 = 1;
    pub const EPOLL_CTL_DEL: i32 = 2;
    pub const EPOLLIN: i32 = 0x1;
    pub const EPOLLONESHOT: i32 = 0x40000000;

    /// 由于同一名称多次使用，可能会造成混淆，但我们有一个 `Event` 结构体。
    /// 此结构体将文件描述符和一个名为 `events` 的字段绑定在一起。`events` 字段保存了哪些事件已准备好用于该文件描述符的信息。
    pub struct Event {
        events: u32, // 用户注册的事件类型 比如 EPOLLIN | EPOLLONESHOT 表示对Read事件感兴趣并且在第一个事件之后从队列中移除所有兴趣
        epoll_data: usize, // 用户数据，我们可以放置一个用来标识事件的数字 Token
    }

    impl Event {
        pub fn new(events: i32, id: usize) -> Self {
            Event {
                events: events as u32,
                epoll_data: id,
            }
        }

        pub fn data(&self) -> usize {
            self.epoll_data
        }
    }

    // linux系统调用
    #[link(name = "c")]
    extern "C" {
        pub fn epoll_create(size: i32) -> i32;
        pub fn close(fd: i32) -> i32;
        pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut Event) -> i32;
        pub fn epoll_wait(epfd: i32, events: *mut Event, maxevents: i32, timeout: i32) -> i32;
        pub fn eventfd(initva: u32, flags: i32) -> i32;
    }
}

fn epoll_create() -> io::Result<i32> {
    let res = unsafe { ffi::epoll_create(1) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(res)
    }
}

fn close(fd: i32) -> io::Result<()> {
    let res = unsafe { ffi::close(fd) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: &mut Event) -> io::Result<()> {
    let res = unsafe { ffi::epoll_ctl(epfd, op, fd, event) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn epoll_wait(epfd: i32, events: &mut [Event], maxevents: i32, timeout: i32) -> io::Result<i32> {
    let res = unsafe { ffi::epoll_wait(epfd, events.as_mut_ptr(), maxevents, timeout) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(res)
    }
}

fn eventfd(initva: u32, flags: i32) -> io::Result<i32> {
    let res = unsafe { ffi::eventfd(initva, flags) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(res)
    }
}