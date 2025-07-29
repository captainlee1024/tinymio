use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::{io, thread};
/// 集成测试驱动开发
/// 需求
/// 1. 在等待事件时阻塞当前线程
/// 2. 跨操作系统使用相同的API
/// 3. 能够从与我们运行主循环不同的线程注册感兴趣的事件

/// 1. 阻塞当前线程
///     a. 调用主事件队列实例Poll和阻塞方法poll()
///     b. 标识事件的事物称为 Token
///     c. 需要一个表示Event的结构
///
/// 2. 适用所有平台的一个API
///
/// 3. 从不同的线程注册兴趣事件
///     a. 需要一个Registrator知道我们的事件队列是否存在并切能够
/// 将实例发送Registrator到另一个线程的方法
use tinymio::{Events, Interests, Poll, Registrator, TcpStream};

const TEST_TOKEN: usize = 10; // Hard coded for this test only

#[test]
fn proposed_api() {
    // 1. 创建Reactor到Executor通信的channel
    // 用于Reactor监听到操作系统事件对列中事件发生后通知Executor继续执行后续逻辑
    let (evt_sender, evt_receiver) = channel();
    // 2. 创建Reactor: 用于注册事件到操作系统和持续监听就绪事件
    let mut reactor = Reactor::new(evt_sender);
    // 3. 创建Executor:
    //  - 用于记录注册的事件的Token和后续处理逻辑
    //  - 用于接收Reactor通知触发的事件对应的Token, 然后取出对应的后续逻辑进行执行
    let mut executor = Excutor::new(evt_receiver);

    let mut stream = TcpStream::connect("127.0.0.1:9527").unwrap();
    let request = format!(
        "GET /delay/{}/url/http://delay.com HTTP/1.1\r\n\
             Host: localhost\r\n\
             Connection: close\r\n\
             \r\n",
        2000,
    );

    stream
        .write_all(request.as_bytes())
        .expect("Stream write err.");

    let registrator = reactor.registrator();
    // 注册对stream感兴趣的(read)事件
    // 4. 注册事件
    registrator
        .register(&mut stream, TEST_TOKEN, Interests::READABLE)
        .expect("registration err.");

    // 把读取stream内容后续处理逻辑封装到函数里托管给executor
    // 5. 记录事件对应的事件源标识: Token, 和后续处理逻辑
    // 这里就相当于挂起了我们等待response的后续处理逻辑，并没有在占用资源了
    // 但是真正的操作系统挂起是 epoll_wait, kqueue的监听事件响应的地方让出线程的
    // 这里是运行时层面的让出，好继续注册其他事件或者其他不需要阻塞持续执行的任务
    executor.suspend(TEST_TOKEN, move || {
        let mut buffer = String::new();
        stream.read_to_string(&mut buffer).unwrap();
        assert!(!buffer.is_empty(), "Got an empty buffer");
        println!("Got {}", buffer);
        registrator.close_loop().expect("close loop err.");
    });

    // executor开始监听之前注册的感兴趣的事件通知，收到通知后从自己托管的
    // 后续处理函数中找到对应的函数执行
    executor.block_on_all();
}

struct Reactor {
    handle: Option<JoinHandle<()>>,
    registrator: Option<Registrator>,
}

impl Reactor {
    fn new(evt_sender: Sender<usize>) -> Reactor {
        let mut poll = Poll::new().unwrap();
        let registrator = poll.registrator();

        let handle = thread::spawn(move || {
            let mut events = Events::with_capacity(1024);
            loop {
                println!("Waiting! {:?}", poll);
                match poll.poll(&mut events, Some(200)) {
                    Ok(..) => (),
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => break,
                    Err(e) => panic!("Poll error: {:?}, {}", e.kind(), e),
                };

                for event in &events {
                    let event_token = event.id();
                    evt_sender.send(event_token).expect("Send event_token err.");
                }
            }
        });

        Reactor {
            handle: Some(handle),
            registrator: Some(registrator),
        }
    }

    fn registrator(&mut self) -> Registrator {
        self.registrator.take().unwrap()
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        let handle = self.handle.take().unwrap();
        handle.join().unwrap();
    }
}

struct Excutor {
    events: Vec<(usize, Box<dyn FnMut()>)>,
    evt_receiver: Receiver<usize>,
}

impl Excutor {
    fn new(evt_receiver: Receiver<usize>) -> Self {
        Excutor {
            events: vec![],
            evt_receiver,
        }
    }

    // 挂起，等待事件完成唤醒
    fn suspend(&mut self, id: usize, f: impl FnMut() + 'static) {
        self.events.push((id, Box::new(f)));
    }

    // 唤醒之前的任务
    fn resume(&mut self, event: usize) {
        let (_, f) = self
            .events
            .iter_mut()
            .find(|(e, _)| *e == event)
            .expect("Couldn't find event.");
        f();
    }

    fn block_on_all(&mut self) {
        // 收到信号，唤醒之前的任务继续执行
        while let Ok(received_token) = self.evt_receiver.recv() {
            assert_eq!(TEST_TOKEN, received_token, "Non matching tokens.");
            self.resume(received_token);
        }
    }
}
