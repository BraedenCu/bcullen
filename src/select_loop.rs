use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use mio::net::TcpStream as MioTcpStream;
use mio::{Events, Interest, Poll, Token};
use crate::request::{HttpRequest, Method, ParseError};
use crate::response::HttpResponse;
use crate::router::{route_request, RouteResult};
use crate::cgi::execute_cgi;
use crate::server::ServerState;

const BUFFER_SIZE: usize = 8192;
/*
Struct defined for each connection state
*/
struct Connection 
{
    stream: MioTcpStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    state: ConnState,
    created_at: Instant,
    #[allow(dead_code)]
    peer_addr: String,
    peer_ip: String,
    peer_port: u16,
    keep_alive: bool,
}

#[derive(Debug, PartialEq)]
enum ConnState 
{
    ReadingRequest,
    WritingResponse,
    Done,
}

impl Connection 
{
    fn new(stream: MioTcpStream, peer_addr: String) -> Self 
    {
        let (peer_ip, peer_port) = parse_peer_addr(&peer_addr);
        Connection 
        {
            stream,
            read_buf: Vec::with_capacity(BUFFER_SIZE),
            write_buf: Vec::new(),
            state: ConnState::ReadingRequest,
            created_at: Instant::now(),
            peer_addr,
            peer_ip,
            peer_port,
            keep_alive: true,
        }
    }

    fn reset_for_keepalive(&mut self) 
    {
        self.read_buf.clear();
        self.write_buf.clear();
        self.state = ConnState::ReadingRequest;
        self.created_at = Instant::now();
    }
}

/*
Start server w/ n select/poll event loops note asymmetric design chosen. We will have
each loop handle its own connections and the main thread will accept and route 
connections via channels. Responsibilities are not interchangable, main thread does just
one job (accepting + distributing) and teh workers do diff jobs (process events).
We use round robin distribution. Tradeoff is that theres a single bottleneck for accepting
connections.
*/
pub fn run_select(state: Arc<ServerState>, n_loops: usize) 
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
    println!("Select loops: {}", n_loops);
    for vh in &state.config.virtual_hosts 
    {
        println!(
            "  VirtualHost: {} -> {}",
            vh.server_name,
            vh.document_root.display()
        );
    }

    // create channels to dispatch connections to event loops
    let mut senders = Vec::new();
    let mut loop_handles = Vec::new();

    for id in 0..n_loops 
    {
        let (tx, rx) = std::sync::mpsc::channel::<MioTcpStream>();
        senders.push(tx);

        let state = Arc::clone(&state);
        let handle = thread::Builder::new()
            .name(format!("select-loop-{}", id))
            .spawn(move || {
                event_loop(id, rx, &state);
            })
            .unwrap_or_else(|e| {
                eprintln!("Failed to spawn select loop {}: {}", id, e);
                std::process::exit(1);
            });
        loop_handles.push(handle);
    }

    // round robin to event loops
    let mut next_loop = 0;
    loop 
    {
        if state.shutdown.load(Ordering::Relaxed) 
        {
            break;
        }

        match listener.accept() {
            Ok((stream, _addr)) => 
            {
                state.active_connections.fetch_add(1, Ordering::Relaxed);

                stream.set_nonblocking(true).ok();
                let mio_stream = MioTcpStream::from_std(stream);

                if senders[next_loop].send(mio_stream).is_err() {
                    state.active_connections.fetch_sub(1, Ordering::Relaxed);
                    break;
                }

                next_loop = (next_loop + 1) % n_loops;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => 
            {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(e) => 
            {
                if !state.shutdown.load(Ordering::Relaxed) {
                    eprintln!("Accept error: {}", e);
                }
                break;
            }
        }
    }

    state.accepting.store(false, Ordering::Relaxed);
    drop(senders);

    println!("Waiting for select loops to finish...");
    for handle in loop_handles {
        let _ = handle.join();
    }
    println!("Server shut down.");
}

/*
single event loop handling many connectios via mia:poll. we don't want to 
block all connections individually rather we need to let a single event
loop monitor may connections at once while ONLY reacting when a socket is actually 
ready for read/write. Multiplexing.
*/
fn event_loop(
    id: usize,
    new_conn_rx: std::sync::mpsc::Receiver<MioTcpStream>,
    state: &ServerState,
) 
{
    let mut poll = Poll::new().expect("Failed to create Poll");
    let mut events = Events::with_capacity(256);
    let mut connections: HashMap<Token, Connection> = HashMap::new();
    let mut next_token: usize = 0;
    let timeout_duration = Duration::from_secs(3);

    loop 
    {
        loop 
        {
            match new_conn_rx.try_recv() 
            {
                Ok(mut stream) => 
                {
                    let token = Token(next_token);
                    next_token += 1;

                    let peer_addr = stream
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|_| "unknown".to_string());

                    poll.registry()
                        .register(&mut stream, token, Interest::READABLE)
                        .ok();

                    connections.insert(token, Connection::new(stream, peer_addr));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => 
                {
                    // done accepting, empty connections
                    if connections.is_empty() 
                    {
                        return;
                    }
                    break;
                }
            }
        }

        if connections.is_empty() && new_conn_rx.try_recv().is_err() 
        {
            if state.shutdown.load(Ordering::Relaxed) 
            {
                return;
            }
        }

        let poll_timeout = Some(Duration::from_millis(100));
        if let Err(e) = poll.poll(&mut events, poll_timeout) 
        {
            if e.kind() == std::io::ErrorKind::Interrupted 
            {
                continue;
            }
            eprintln!("Poll error in loop {}: {}", id, e);
            break;
        }

        let mut to_remove = Vec::new();

        for event in events.iter() 
        {
            let token = event.token();

            if let Some(conn) = connections.get_mut(&token) 
            {
                match conn.state 
                {
                    ConnState::ReadingRequest => 
                    {
                        if handle_read(conn, state, &poll) 
                        {
                            if conn.state == ConnState::Done 
                            {
                                to_remove.push(token);
                            } 
                            else if conn.state == ConnState::WritingResponse 
                            {
                                // reregister as writable
                                poll.registry()
                                    .reregister(
                                        &mut conn.stream,
                                        token,
                                        Interest::WRITABLE,
                                    )
                                    .ok();
                            }
                        }
                    }
                    ConnState::WritingResponse => 
                    {
                        if handle_write(conn, &poll) 
                        {
                            if conn.state == ConnState::Done 
                            {
                                to_remove.push(token);
                            } 
                            else if conn.state == ConnState::ReadingRequest 
                            {
                                // keep alive process we reregister on read
                                poll.registry()
                                    .reregister(
                                        &mut conn.stream,
                                        token,
                                        Interest::READABLE,
                                    )
                                    .ok();
                            }
                        }
                    }
                    ConnState::Done => 
                    {
                        to_remove.push(token);
                    }
                }
            }
        }

        let now = Instant::now();
        for (token, conn) in connections.iter() 
        {
            if conn.state == ConnState::ReadingRequest
                && now.duration_since(conn.created_at) > timeout_duration
            {
                to_remove.push(*token); // send 404 timeout time
            }
        }

        for token in to_remove 
        {
            if let Some(mut conn) = connections.remove(&token) 
            {
                // clean up connections
                poll.registry().deregister(&mut conn.stream).ok();
                state.active_connections.fetch_sub(1, Ordering::Relaxed);
            }
        }

        // check for shutdown
        if state.shutdown.load(Ordering::Relaxed) && connections.is_empty() 
        {
            return;
        }
    }
}

/*
handle readable events, will ret true if state transition has occured
*/
fn handle_read(conn: &mut Connection, state: &ServerState, _poll: &Poll) -> bool 
{
    let mut buf = [0u8; BUFFER_SIZE];

    loop 
    {
        match conn.stream.read(&mut buf) 
        {
            Ok(0) => 
            {
                conn.state = ConnState::Done;
                return true;
            }
            Ok(n) => 
            {
                conn.read_buf.extend_from_slice(&buf[..n]);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => 
            {
                break;
            }
            Err(_) => 
            {
                conn.state = ConnState::Done;
                return true;
            }
        }
    }

    if let Some(header_end) = find_header_end(&conn.read_buf) 
    {
        // parse req from the buffer
        let request_data = conn.read_buf.clone();
        match parse_request_from_buffer(&request_data, header_end) 
        {
            Ok((request, consumed)) => 
            {
                let connection_header = request
                    .header("connection")
                    .unwrap_or("keep-alive")
                    .to_lowercase();
                conn.keep_alive = connection_header != "close";
                let response_bytes = process_request(request, state, &conn.peer_ip, conn.peer_port);
                conn.write_buf = response_bytes;
                conn.read_buf.drain(..consumed);
                conn.state = ConnState::WritingResponse;

                return true;
            }
            Err(ParseError::Incomplete) => 
            {
                // post body not fully received yet so we gotta stay in ReadingRequest
                return false;
            }
            Err(_) => 
            {
                let resp = HttpResponse::bad_request("Malformed request");
                conn.write_buf = resp.serialize();
                conn.keep_alive = false;
                conn.state = ConnState::WritingResponse;
                return true;
            }
        }
    }

    false
}

/*
handle writable event. ret true if state transition occurs
*/
fn handle_write(conn: &mut Connection, _poll: &Poll) -> bool 
{
    if conn.write_buf.is_empty() 
    {
        if conn.keep_alive 
        {
            conn.reset_for_keepalive();
            return true;
        } 
        else 
        {
            conn.state = ConnState::Done;
            return true;
        }
    }

    match conn.stream.write(&conn.write_buf) 
    {
        Ok(n) => 
        {
            conn.write_buf.drain(..n);
            if conn.write_buf.is_empty() 
            {
                if conn.keep_alive 
                {
                    conn.reset_for_keepalive();
                    return true;
                } 
                else 
                {
                    conn.state = ConnState::Done;
                    return true;
                }
            }
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(_) => 
        {
            conn.state = ConnState::Done;
            return true;
        }
    }

    false
}

fn find_header_end(buf: &[u8]) -> Option<usize> 
{
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_request_from_buffer(
    buf: &[u8],
    header_end: usize,
) -> Result<(HttpRequest, usize), ParseError> 
{
    let header_section = &buf[..header_end];
    let header_str =
        std::str::from_utf8(header_section).map_err(|_| ParseError::InvalidRequestLine)?;

    let mut lines = header_str.lines();

    let request_line = lines.next().ok_or(ParseError::InvalidRequestLine)?;
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() != 3 
    {
        return Err(ParseError::InvalidRequestLine);
    }

    let method = match parts[0] 
    {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => return Err(ParseError::InvalidMethod),
    };

    let uri = parts[1].to_string();
    let version = parts[2].to_string();

    if !version.starts_with("HTTP/") 
    {
        return Err(ParseError::InvalidVersion);
    }

    let mut headers = std::collections::HashMap::new();
    for line in lines 
    {
        if line.is_empty() 
        {
            break;
        }
        if let Some((key, value)) = line.split_once(':') 
        {
            headers.insert(key.trim().to_lowercase(), value.trim().to_string());
        }
    }

    let mut consumed = header_end + 4;

    let mut body = Vec::new();
    if method == Method::Post 
    {
        if let Some(len_str) = headers.get("content-length") 
        {
            if let Ok(len) = len_str.parse::<usize>() 
            {
                let body_end = consumed + len;
                if buf.len() >= body_end 
                {
                    body = buf[consumed..body_end].to_vec();
                    consumed = body_end;
                } 
                else 
                {
                    return Err(ParseError::Incomplete);
                }
            }
        }
    }

    Ok((
        HttpRequest 
        {
            method,
            uri,
            version,
            headers,
            body,
        },
        consumed,
    ))
}

/*
takes fully parsed HttpRequest and returns raw response bytes Vec<u8> to be
written back out to the client. 
*/
fn process_request(
    request: HttpRequest,
    state: &ServerState,
    peer_ip: &str,
    peer_port: u16,
) -> Vec<u8> 
{
    let connection_header = request
        .header("connection")
        .unwrap_or("keep-alive")
        .to_lowercase();
    let keep_alive = connection_header != "close";

    /*
    runs inside nonblocking event loop checks during request ready
    event calback. returns serialized response bytes which the event
    loop then queues for nonblocking writes back. At this point load 
    balancer would come in to determine which backend server gets
    the next client req.
     */
    if request.path() == "/load" && request.method == Method::Get 
    {
        let mut resp = if state.accepting.load(Ordering::Relaxed) && !state.is_overloaded() {
            let mut r = HttpResponse::ok();
            r.set_body(b"OK".to_vec(), "text/plain");
            r
        } 
        else 
        {
            HttpResponse::service_unavailable()
        };
        resp.set_header(
            "Connection",
            if keep_alive { "keep-alive" } else { "close" },
        );
        return resp.serialize();
    }

    let hostname = request
        .header("host")
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    let vhost = state.config.find_host(hostname);

    match route_request(request, vhost) 
    {
        RouteResult::Response(mut resp) => 
        {
            resp.set_header(
                "Connection",
                if keep_alive { "keep-alive" } else { "close" },
            );
            resp.serialize()
        }
        RouteResult::Cgi 
        {
            script_path,
            request,
        } => 
        {
            let resp = execute_cgi(
                &script_path,
                &request,
                vhost,
                peer_ip,
                peer_port,
                state.config.listen_port,
            );
            if resp.headers.is_empty() 
            {
                // raw cgi response
                resp.body
            } 
            else 
            {
                resp.serialize()
            }
        }
    }
}

fn parse_peer_addr(addr: &str) -> (String, u16) 
{
    if let Some(bracket_end) = addr.rfind(']') 
    {
        let ip = &addr[..=bracket_end];
        let port = addr[bracket_end + 1..]
            .trim_start_matches(':')
            .parse()
            .unwrap_or(0);
        (ip.to_string(), port)
    } 
    else if let Some(colon) = addr.rfind(':') 
    {
        let ip = &addr[..colon];
        let port = addr[colon + 1..].parse().unwrap_or(0);
        (ip.to_string(), port)
    } 
    else 
    {
        (addr.to_string(), 0)
    }
}
