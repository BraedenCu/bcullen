mod cgi;
mod config;
mod connection;
mod mime;
mod request;
mod response;
mod router;
mod select_loop;
mod server;
mod util;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::io::{self, BufRead};

use config::{ProcessingMode, ServerConfig};
use server::ServerState;

fn main() {
    // grab stdin arguments
    let args: Vec<String> = std::env::args().collect();

    // parse configuration
    let config_path = parse_args(&args).unwrap_or_else(|| {
        eprintln!("Usage: {} -config <config_file>", args[0]);
        std::process::exit(1);
    }); 
    let config = ServerConfig::parse(&config_path).unwrap_or_else(|e| {
        eprintln!("Configuration error: {}", e);
        std::process::exit(1);
    });

    // arc = atomically reference, thread safety mechanism built into rust.
    // One ServerState instance needs to be read by multiple threads at the same time
    // so we must leverage a smart pointer (arc) to hand server state to multiple threads
    // while preventing race conditions. 
    // note: atomic means indivisible operation and thus cannot be interrupted in partially completed state.
    let state = Arc::new(ServerState::new(config.clone()));

    // spawn management thread --> sits in a loop reading stdin. this is blocking call
    // because we need a graceful shutdown to avoid disrupting active connections. 
    let mgmt_state = Arc::clone(&state);
    thread::Builder::new()
        .name("management".to_string())
        .spawn(move || {
            management_terminal(&mgmt_state);
        })
        .expect("Failed to spawn management thread");

    // carries choice of processing mode (ether threaded or select loops)
    match &config.processing_mode {
        // Spawn nThreads, keeping N workers idle while connections exist on shared channel,
        // think waiters rotating between tables at a restaurant. Blocked on slow client, as thread
        // sleeps. Apache
        ProcessingMode::Threads(n) => {
            server::run_threaded(state, *n);
        }
        // N event loops all multiplex many connections w/ mio, which wrapps epoll. Nice
        // because it is way more scalable under high load. Will not be blocked on a slow client. 
        // Nginx
        ProcessingMode::SelectLoops(n) => {
            select_loop::run_select(state, *n);
        }
    }
}

fn parse_args(args: &[String]) -> Option<String> {
    let mut i = 1;
    while i < args.len() {
        if args[i] == "-config" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
        i += 1;
    }
    None
}

/*
Approach: To ensure graceful shutdown, we perform atomic updates to flag the system
to initiate said shutdown. The sequencing will look like so:

operator types "shutdown" into stdin
                v
management_termainl sets shutdown=true, accepting=false
                v
accept loop in run_threaded sees shutdown=true flip, stops calling accept()
                v
accept loop drops mpsc (multi-producer, single-consumer) sender and channel closes
                v
worker threads get error on channel.recv(), breaking out of their loops
                v
main threads join all workers and process exists
*/
fn management_terminal(state: &ServerState) {
    let stdin = io::stdin();
    println!("Management terminal ready. Commands: shutdown, status");

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let cmd = line.trim().to_lowercase();

        match cmd.as_str() {
            // Graceful shutdown, we flip atomic flags (shutdown) and cleanly exit. Critically
            // the server workers will keep running until they complete in flight connections. 
            "shutdown" => {
                println!("Initiating graceful shutdown...");
                state.shutdown.store(true, Ordering::Relaxed);
                state.accepting.store(false, Ordering::Relaxed);
                break;
            }
            // Simply read the live counters from ServerState and print them, we are good to do this
            // because the atomic updates are safe (indivisible)
            "status" => {
                let active = state.active_connections.load(Ordering::Relaxed);
                let accepting = state.accepting.load(Ordering::Relaxed);
                println!(
                    "Active connections: {}, Accepting: {}, Overloaded: {}",
                    active,
                    accepting,
                    state.is_overloaded()
                );
            }
            "" => {}
            _ => {
                println!("Unknown command: '{}'. Available: shutdown, status", cmd);
            }
        }
    }
}
