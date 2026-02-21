use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use crate::config::{ProcessingMode, ServerConfig};
use crate::connection::handle_connection;

/*
Shared server state, accessible from all threads
*/
pub struct ServerState 
{
    pub config: ServerConfig,
    pub shutdown: AtomicBool,
    pub active_connections: AtomicUsize,
    pub accepting: AtomicBool,
}

impl ServerState 
{
    pub fn new(config: ServerConfig) -> Self 
    {
        ServerState 
        {
            config,
            shutdown: AtomicBool::new(false),
            active_connections: AtomicUsize::new(0),
            accepting: AtomicBool::new(true),
        }
    }

    pub fn is_overloaded(&self) -> bool 
    {
        // overloaded if more connections than 10x thread count avail
        let max = match &self.config.processing_mode 
        {
            ProcessingMode::Threads(n) => n * 10,
            ProcessingMode::SelectLoops(n) => n * 10,
        };
        self.active_connections.load(Ordering::Relaxed) >= max
    }
}

pub fn run_threaded(state: Arc<ServerState>, n_threads: usize) 
{
    let addr = format!("0.0.0.0:{}", state.config.listen_port);
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        eprintln!("Failed to bind to {}: {}", addr, e);
        std::process::exit(1);
    });

    listener
        .set_nonblocking(true)
        .expect("Failed to set non-blocking");

    println!("Server listening on {}", addr);
    println!("Thread pool size: {}", n_threads);
    for vh in &state.config.virtual_hosts 
    {
        println!(
            "  VirtualHost: {} -> {}",
            vh.server_name,
            vh.document_root.display()
        );
    }

    let (sender, receiver) = mpsc::channel::<TcpStream>();
    let receiver = Arc::new(Mutex::new(receiver));

    let mut workers = Vec::new();
    for id in 0..n_threads 
    {
        let receiver = Arc::clone(&receiver);
        let state = Arc::clone(&state);
        let handle = thread::Builder::new()
            .name(format!("worker-{}", id))
            .spawn(move || 
                {
                loop 
                {
                    let stream = {
                        let lock = receiver.lock().unwrap();
                        lock.recv()
                    };

                    match stream 
                    {
                        Ok(stream) => 
                        {
                            state.active_connections.fetch_add(1, Ordering::Relaxed);
                            handle_connection(stream, &state);
                            state.active_connections.fetch_sub(1, Ordering::Relaxed);
                        }
                        Err(_) => 
                        {
                            break;
                        }
                    }
                }
            })
            // write to stderr
            .unwrap_or_else(|e| {
                eprintln!("Failed to spawn worker thread {}: {}", id, e);
                std::process::exit(1);
            });
        workers.push(handle);
    }

    loop 
    {
        if state.shutdown.load(Ordering::Relaxed) 
        {
            break;
        }

        match listener.accept() 
        {
            Ok((stream, _addr)) => 
            {
                stream.set_nonblocking(false).ok();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .ok();

                if state.shutdown.load(Ordering::Relaxed) 
                {
                    break;
                }

                if sender.send(stream).is_err() 
                {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => 
            {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => 
            {
                if !state.shutdown.load(Ordering::Relaxed) 
                {
                    eprintln!("Accept error: {}", e);
                }
                break;
            }
        }
    }

    state.accepting.store(false, Ordering::Relaxed);
    drop(sender);

    println!("waiting on workers to finish");
    for handle in workers 
    {
        let _ = handle.join();
    }
    println!("server shut down");
}
