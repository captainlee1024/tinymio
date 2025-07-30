use tinymio::{Events, Interests, Poll, Registrator, TcpStream};
use std::io::{self, Read, Write};
use std::sync::mpsc::channel;
use std::thread;

//  cargo test multiple_registrations -- --nocapture
#[test]
fn multiple_registrations() {
    let mut poll = Poll::new().unwrap();
    let registrator = poll.registrator();
    let (event_tx, event_rx) = channel();
    let mut runtime = Runtime{ events: vec![]};

    let token1 = 10;
    let token2 = 11;

    // 持续监听内核事件队列，并将触发的事件 id 发送给runtime 进行后续处理
    let handle = thread::spawn(move || {
        let mut events = Events::with_capacity(1024);
        loop {
            println!("Polling");
            let will_close = false;
            println!("poll: {:?}", poll);
            match poll.poll(&mut events, Some(200)) {
                Ok(..) => (),
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {
                    println!("INTERRUPTED: {}", err);
                    break;
                }
                Err(err) => panic!("Poll error: {:?}, {}", err.kind(), err),
            };
            for event in &events {
                let event_id = event.id();
                println!("Got Event: {:?}", event_id);
                event_tx.send(event_id).unwrap();
            }

            if will_close {
                break;
            }
        }
    });

    // 初始化两个tcp连接并发送请求
    let mut stream1 = TcpStream::connect("127.0.0.1:9527").unwrap();
    let request1 = format!(
        "GET /delay/{}/url/http://delay.com HTTP/1.1\r\n\
             Host: localhost\r\n\
             Connection: close\r\n\
             \r\n",
        2000,
    );

    stream1
        .write_all(request1.as_bytes())
        .expect("Stream write err.");

    let mut stream2 = TcpStream::connect("127.0.0.1:9527").unwrap();
    let request2 = format!(
        "GET /delay/{}/url/http://delay.com HTTP/1.1\r\n\
             Host: localhost\r\n\
             Connection: close\r\n\
             \r\n",
        2000,
    );

    stream2
        .write_all(request2.as_bytes())
        .expect("Stream write err.");

    // 注册两个event到内核事件队列
    registrator.register(&mut stream1, token1, Interests::READABLE).unwrap();
    registrator.register(&mut stream2, token2, Interests::READABLE).unwrap();

    // 注册两个socket可读后的后续处理逻辑
    runtime.spawn(token1, move || {
        let mut buffer = String::new();
        stream1.read_to_string(&mut buffer).unwrap();
        println!("Get response from stream1: {}", buffer);
    });

    runtime.spawn(token2, move || {
        let mut buffer = String::new();
        stream2.read_to_string(&mut buffer).unwrap();
        println!("Get response from stream2: {}", buffer);
    });

    // 启动runtime main loop, 持续接收registrator的通知，唤醒对应的代码进行处理
    let mut counter = 0;
    while let Ok(received_evt_id) = event_rx.recv() {
        counter += 1;
        println!("Received Evt id: {}", received_evt_id);
        runtime.run(received_evt_id);
        if counter == 2 {
            registrator.close_loop().unwrap()
        }
    }

    handle.join().unwrap();
    println!("EXITING");
}

struct Runtime {
    events: Vec<(usize, Box<dyn FnMut()>)>,
}

impl Runtime {
    fn spawn(&mut self, event_id: usize, f: impl FnMut() + 'static) {
        self.events.push((event_id, Box::new(f)));
    }

    fn run(&mut self, event_id: usize) {
        println!("Running event {}", event_id);
        let (_, f) = self.events.iter_mut().find(|(e, _)| *e == event_id).expect("Couldn't find event");
        f();
    }
}