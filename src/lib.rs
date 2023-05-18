use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Event, Registrator, Selector, TcpStream};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{Event, Registrator, Selector, TcpStream};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{Event, Registrator, Selector, TcpStream};

pub type Events = Vec<Event>;
pub type Token = usize;

#[derive(Debug)]
pub struct Poll {
    registry: Registry,
    is_poll_dead: Arc<AtomicBool>,
}

impl Poll {
    pub fn new() -> io::Result<Poll> {
        Selector::new().map(|selector| Poll {
            registry: Registry { selector },
            is_poll_dead: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn registrator(&self) -> Registrator {
        self.registry
            .selector
            .registrator(self.is_poll_dead.clone())
    }
    pub fn poll(&mut self, events: &mut Events, timeout_ms: Option<i32>) -> io::Result<usize> {
        let timeout = timeout_ms.map(|n| if n < 0 { 0 } else { n });
        loop {
            let res = self.registry.selector.select(events, timeout);
            match res {
                Ok(()) => break,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => (),
                Err(e) => return Err(e),
            };
        }

        if self.is_poll_dead.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Poll closed."));
        }
        Ok(events.len())
    }
}

#[derive(Debug)]
pub struct Registry {
    selector: Selector,
}

const WRITABLE: u8 = 0b0000_0001;
const READABLE: u8 = 0b0000_0010;

pub struct Interests(u8);
impl Interests {
    pub const READABLE: Interests = Interests(READABLE);
    pub const WRITABLE: Interests = Interests(WRITABLE);

    pub fn is_readable(&self) -> bool {
        self.0 & READABLE != 0
    }

    pub fn is_writable(&self) -> bool {
        self.0 & WRITABLE != 0
    }
}
