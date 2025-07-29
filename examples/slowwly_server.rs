use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, mpsc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

/// 用于测试的延时服务关闭了, 所以在执行主程序之前请先启动该服务模拟网络延时
/// cargo run --example slowwly_server --quiet
fn main() {
    let addr = "127.0.0.1:9527";
    let listener = TcpListener::bind(addr).unwrap();
    let pool = ThreadPool::new(4);

    for stream in listener.incoming() {
        let stream = stream.unwrap();
        pool.execute(|| {
            handle_connection(stream)
        });

        // thread::spawn(|| {
        //     handle_connection(stream)
        // });
    }
}

fn handle_connection(mut stream: TcpStream) {
    let buf_reader = BufReader::new(&mut stream);

    // 展示request
    let http_request: Vec<_> = buf_reader
        .lines()
        .map(|result| result.unwrap())
        .take_while(|line| !line.is_empty())
        .collect();
    println!("Request: {:#?}", http_request);
    println!("=====================start handle");
    let split: Vec<_> = http_request[0].split(" ").collect();
    // println!("split: {:#?}", split);

    let route: Vec<_> = split[1]
        .split("/")
        .filter(|result| !result.eq(&""))
        .collect();
    // println!("route: {:#?}", route);

    let delay_ms_str = route[1];

    // println!("delay_ms_str: {}", delay_ms_str);
    let delay_ms = delay_ms_str.parse::<u64>().unwrap();

    // println!("delay ms: {}", delay_ms);

    let delay_second = Duration::from_millis(delay_ms);

    sleep(delay_second);

    let response = "HTTP/1.1 200 OK\r\n\r\n";
    println!("=====================done");
    stream.write_all(response.as_bytes()).unwrap();
}

pub struct ThreadPool{
    workers: Vec<Worker>,
    sender: mpsc::Sender<Job>,
}

impl ThreadPool {
    pub fn new(size: usize) -> ThreadPool {
        assert!( size > 0);

        let (sender, receiver) = mpsc::channel();
        let mut workers = Vec::with_capacity(size);

        let receiver = Arc::new(Mutex::new(receiver));
        for id in 0..size {
            workers.push(Worker::new(id, Arc::clone(&receiver))) ;
        }

        ThreadPool {workers, sender}
    }

    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let job = Box::new(f);

        self.sender.send(job).unwrap();
    }
}

pub struct Worker {
    id: usize,
    thread: thread::JoinHandle<()>,
}

impl Worker {
    fn new(id: usize, receiver: Arc<Mutex<mpsc::Receiver<Job>>>) -> Worker {
        let thread = thread::spawn(move || loop {
            let job = receiver.lock().unwrap().recv().unwrap();

            println!("Worker {id} got a job; executing.");

            job();
        });

        Worker{id, thread}
    }
}

type Job = Box<dyn FnOnce() + Send + 'static>;