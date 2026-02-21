# CPSC433 Assignment One: Rust HTTP/1.x Protocol and Server

___

## Program Flow

```text
                          -----------------------------------
                          │           main.rs               │
                          │  LI parsing, ServerConfig load  │
                          │ Spawn management terminal thread│
                          -----------------------------------
                                      │
                     -----------------------------------
                     │          ProcessingMode?        │
                     v                                 v
          --------------------              -----------------------
          │   server.rs      │              │   select_loop.rs    │
          │  Thread Pool     │              │  Event Loop (mio)   │
          │  (nThreads N)    │              │  (nSelectLoops N)   │
          --------------------              -----------------------
                 │                                 │
                 │  TcpListener::accept()          │  TcpListener::accept()
                 │  (non-blocking)                 │  (non-blocking)
                 │                                 │
                 v                                 v
     -------------------------       --------------------------------
     │  mpsc channel dispatch│       │  Round-robin dispatch via    │
     │  to N worker threads  │       │  mpsc channels to N loops    │
     -------------------------       --------------------------------
             │                              │
             v                              v
  ------------------------      ------------------------------------
  │  connection.rs       │      │  Per-connection state machine    │
  │  handle_connection() │      │  ConnState:                      │
  │  (blocking I/O,      │      │    ReadingRequest                │
  │   loop for           │      │    WritingResponse               │
  │   keep-alive)        │      │    Done                          │
  ------------------------      │  mio::Poll for readiness events  │
            │                   ------------------------------------
            │                          │
            ----------------------------
                     │
                     v
    ----------------------------------------
    │          Request Pipeline            │
    │                                      │
    │  1. request.rs :: parse_request()    │
    │     Parse method, URI, headers, body │
    │                                      │
    │  2. /load check (special endpoint)   │
    │     200 if healthy, 503 if overloaded│
    │                                      │
    │  3. config.rs :: find_host()         │
    │     Match Host header → VirtualHost  │
    │                                      │
    │  4. router.rs :: route_request()     │
    │     ├─ resolve_path()                │
    │     │   ├─ Map URI → DocumentRoot    │
    │     │   ├─ Directory → index.html    │
    │     │   └─ Mobile → index_m.html     │
    │     ├─ Path traversal protection     │
    │     ├─ .htaccess auth check          │
    │     └─ Executable? → CGI or Static   │
    │                                      │
    │  5a. Static file:                    │
    │      ├─ mime.rs :: mime_from_ext()   │
    │      ├─ Accept header validation     │
    │      ├─ If-Modified-Since → 304      │
    │      └─ Read file → 200 response     │
    │                                      │
    │  5b. CGI script:                     │
    │      ├─ cgi.rs :: execute_cgi()      │
    │      ├─ Set RFC 3875 env vars        │
    │      ├─ Fork/exec, pipe stdin/out    │
    │      └─ Chunked transfer encoding    │
    │                                      │
    │  6. response.rs :: serialize()       │
    │     Build HTTP/1.1 wire format       │
    ----------------------------------------

    ----------------------------------------
    │         Shared State (Arc)           │
    │                                      │
    │  server.rs :: ServerState            │
    │  ├─ config: ServerConfig             │
    │  ├─ shutdown: AtomicBool             │
    │  ├─ accepting: AtomicBool            │
    │  └─ active_connections: AtomicUsize  │
    ----------------------------------------

    ----------------------------------------
    │         Source File Map              │
    │                                      │
    │  main.rs ------- Entry, CLI, mgmt    │
    │  config.rs ----- Apache-style parse  │
    │  server.rs ----- Thread pool mode    │
    │  select_loop.rs  Event loop mode     │
    │  connection.rs - Blocking handler    │
    │  request.rs ---- HTTP request parse  │
    │  response.rs --- HTTP response build │
    │  router.rs ----- URL routing, auth   │
    │  cgi.rs -------- CGI execution       │
    │  mime.rs ------- MIME type lookup    │
    │  util.rs --------Date formatting     │
    ----------------------------------------
```

___

## Running the program

The server listens on the port specified in the config (default: 6789). A management terminal is available on stdin. Simply type "shutdown" for graceful shutdown or "status" to see active connections.

```bash
# build 
cargo build

# thread pool mode
cargo run -- -config http-meeting/test-http-threads.conf

# select loop mode
cargo run -- -config http-meeting/test-http.conf
```

### Test Cases

With the server running, tests should be run from a separate terminal

```bash
# Target 1: Basic GET (expect 200 with Date, Server, Content-Type, Content-Length, Last-Modified)
curl -v http://localhost:6789/index.html
curl -v http://localhost:6789/index.txt

# Target 2: Timeout (connect via telnet, wait without sending — disconnects after 3s)
telnet localhost 6789

# Target 3: Virtual host routing (should return different content per host)
curl -v -H "Host: host1.cs.yale.edu" http://localhost:6789/index.html
curl -v -H "Host: host2.cs.yale.edu" http://localhost:6789/index.html

# Target 4: Accept header validation (first = 406, second = 200)
curl -v -H "Accept: a/b" http://localhost:6789/index.html
curl -v -H "Accept: */*" http://localhost:6789/index.html

# Target 5: Mobile User-Agent detection (should serve index_m.html)
curl -v -H "User-Agent: my-iPhone" http://localhost:6789/

# Target 6: If-Modified-Since (200 for old date, 304 for future date)
curl -v -H "If-Modified-Since: Thu, 14 Dec 2000 19:29:46 GMT" http://localhost:6789/index.html
curl -v -H "If-Modified-Since: Thu, 14 Dec 2050 19:29:46 GMT" http://localhost:6789/index.html

# Target 7: Connection keep-alive vs close
curl -v -H "Connection: close" http://localhost:6789/index.html
curl -v -H "Connection: keep-alive" http://localhost:6789/index.html

# Target 8: Basic authentication (401 without creds, 200 with creds)
curl -v http://localhost:6789/protect/index.html
curl -v http://cs434:passw0rd.@localhost:6789/protect/index.html

# Target 9: CGI POST
curl -v -X POST -H "Content-Type: application/x-www-form-urlencoded" \
  -d "param1=val&param2=val" http://localhost:6789/test-cgi.cgi

# Target 10: Graceful shutdown
#   Terminal 1: curl -v --limit-rate 2K http://localhost:6789/data.txt
#   Terminal 2 (server): type "shutdown" — server finishes the download then exits

# Target 12: Load endpoint (200 = healthy, 503 = overloaded)
curl -v http://localhost:6789/load

```

### Part d: Comparisons (Written Solutions)

#### Part d.netty: Netty Design

Netty is Java async IO framework used by many; see for example use cases. Please read Netty user's guide and code and answer the following questions:

**a. Netty provides multiple event loop implementations. In a typical server channel setting, two event loop groups are created, with one typically called the boss group and the second worker group. What are they? How does Netty achieve synchronization among them?**

Note that Netty has a universal asynchronous I/O interface called a Channel. The channel abstracts away all operations required for point-to-point communication. We have two event loop groups: boss groups and worker groups. The boss event loop group accepts new TCP connections on the listening serer channel. The worker event loop group will handle all I/O operations for the acepted child channels after the boss has already accepted and registered them to the worker. Netty achieves synchronization among them through confining operations on a child within a single thread assigned to the event group responsible for the child. Additionally, the boss to worker thread handoff is not done by sharing a mutable state, rather, the boss schedules into a worker event loop thread using atomic flags (selector.wakeup()) to safely coordinate between flags.

**b. Netty event loop uses an ioRatio based scheduling algorithm. Please describe how it works.**

```java
/**
* Sets the percentage of the desired amount of time spent for I/O in the event loop.  The default value is
* {@code 50}, which means the event loop will try to spend the same amount of time for I/O as for non-I/O tasks.
*/
public void setIoRatio(int ioRatio) {
    if (ioRatio <= 0 || ioRatio > 100) {
        throw new IllegalArgumentException("ioRatio: " + ioRatio + " (expected: 0 < ioRatio <= 100)");
    }
  this.ioRatio = ioRatio;
}

final int ioRatio = this.ioRatio;
if (ioRatio == 100) {
    try {
        processSelectedKeys();
    } finally {
        // Ensure we always run tasks.
        runAllTasks();
    }
} else {
    final long ioStartTime = System.nanoTime();
    try {
        processSelectedKeys();
    } finally {
        // Ensure we always run tasks.
        final long ioTime = System.nanoTime() - ioStartTime;
        runAllTasks(ioTime * (100 - ioRatio) / ioRatio);
    }
}
} catch (Throwable t) {
handleLoopException(t);
}
```

Netty event loop sets a percentage for the amount of time  to be spent on I/O in an event loop, critically this default is set at 50 percent and thus time will be evenly split between I/O and non-I/O activities. The algorithm works as follows: it first checks if ioRatio equals 0, if so: do I/O operations then run all queued tasks with no time budgeting for I/O. Otherwise, measure time spent using I/O and compute a budget for task execution and run tasks up to that time budget.

**c. A major novel, interesting feature of Netty is ChannelPipeline. A pipeline may consist of a list of ChannelHander. Please scan Netty implementation and give a high-level description of how ChannelPipeline is implemented. Compare HTTP Hello World Server and HTTP Snoop Server, what are the handlers that each includes?**

```bash
                                                 I/O Request
                                            via Channel or
                                        ChannelHandlerContext
                                                      |
  +---------------------------------------------------+---------------+
  |                           ChannelPipeline         |               |
  |                                                  \|/              |
  |    +---------------------+            +-----------+----------+    |
  |    | Inbound Handler  N  |            | Outbound Handler  1  |    |
  |    +----------+----------+            +-----------+----------+    |
  |              /|\                                  |               |
  |               |                                  \|/              |
  |    +----------+----------+            +-----------+----------+    |
  |    | Inbound Handler N-1 |            | Outbound Handler  2  |    |
  |    +----------+----------+            +-----------+----------+    |
  |              /|\                                  .               |
  |               .                                   .               |
  | ChannelHandlerContext.fireIN_EVT() ChannelHandlerContext.OUT_EVT()|
  |        [ method call]                       [method call]         |
  |               .                                   .               |
  |               .                                  \|/              |
  |    +----------+----------+            +-----------+----------+    |
  |    | Inbound Handler  2  |            | Outbound Handler M-1 |    |
  |    +----------+----------+            +-----------+----------+    |
  |              /|\                                  |               |
  |               |                                  \|/              |
  |    +----------+----------+            +-----------+----------+    |
  |    | Inbound Handler  1  |            | Outbound Handler  M  |    |
  |    +----------+----------+            +-----------+----------+    |
  |              /|\                                  |               |
  +---------------+-----------------------------------+---------------+
                  |                                  \|/
  +---------------+-----------------------------------+---------------+
  |               |                                   |               |
  |       [ Socket.read() ]                    [ Socket.write() ]     |
  |                                                                   |
  |  Netty Internal I/O Threads (Transport Implementation)            |
  +-------------------------------------------------------------------+
```

ChannelPipeline is implemented like a bidirectional chain, as seen above. Inbound events (data generated by I/O thread) are handled by inbound handlers. Inbound data is read from a remote peer by input operations. Outbound events are handled by the outbound handler, which generates or transforms outbound traffic, namely write requests. DefaultChannelPipeline is linked together using pointers to traverse over the linked list in a bidirectional manner. Every handler is contained in a ChannelHandlerContext node in which events propogate by calling methods on the context which moves into the next appropriate header directionaly (note the bidirectional traversal). The entire purpose of this pipeline is built around ensuring thread safety, allowing threads to perform mutations without worrying about race conditions.

Handlers in HTTP Hello World Server:
HttpServerCodec
HttpContentCompressor
HttpServerExpectContinueHandler
HttpHelloWorldServerHandler

Handlers in HTTP Snoop Server
HttpRequestDecoder
HttpResponseEncoder
HttpSnoopServerHandler

Compare and contrasting of the two:

- Hello world server uses HttpServerCodec which is a single handler that does both decoding and encoding.
- Snoop server splits the codec into separate Decoder and Encoder, but it has the same functionality as the HttpServerCodec
- The hello world server delivers a fixed response and has a simple implementation. No aggregator is needed because the handler only needs minimal request info and additionally it can work on decoded messages. You can optionally add in HttpContentCompressor header to compress the fixed size body.
- The snoop server instead needs access to the entire request body and headers into order to return them back. It includes the HttpObjectAggregator before running the HttpSnoopServerHandler to define the full request object. It does not have a compressor.
- In both server implementations, the last handler (HttpHelloWorldServerHandler vs HttpSnoopServerHandler) is an inbound application handler that determines exactly which responses need to be sent.

**d. Method calls such as bind return ChannelFuture. Please describe how one may implement the sync method of a future.**

ChannelFuture.sync() waits for this future until it is done, and rethrows the cause of the failure if this future failed (according to the netty documentation).  At a high level sync has two main concerns, blocking and error propogation after completion. All sync is doing on ChannelFuture is blocking until the asynchronous operation is finished and if it has failed it will throw an error. One could implement this feature like so:

1. one could check if the channelfuture is already done, if so return immedaitely or throw failure case
2. if not, wait until completion crucially blocking the calling thread
3. after completion, if the channelfuture failed, determine the cause and throw it. If cancelled, throw a cancellation exception and if it succeeded return the channelfuture.

**e. Instead of using ByteBuffer, Netty introduces a data structure called ByteBuf. Please give one key difference between ByteBuffer and ByteBuf.**

ByteBuf has seperate indicies for read and write (readerIndex, writerIndex) so you dont need to flip. Netty thus no longer needs to call java.nio.ByteBuffer.flip() before sending a message in NIO because ByteBuf leverages two pointers, one for read ops and one for write ops. In contrast, the NIO buffer does not provide a way to find out where the message content starts and ends without leveraging the flip method.

#### Part d.nginx: nginx Design

nginx is currently the most-widely used HTTP server. Please read nginx source code, and developer guide to answer the following questions. You can use the developer guide but need to add reference to the source code.

**a. Although nginx has both Master and Worker, the design is the symmetric design that we covered in class: multiple Workers compete on the shared welcome socket (accept). One issue about the design we said in class is that this design does not offer flexible control such as load balance. Please describe how nginx introduces mechanisms to allow certain load balancing among workers? Related with the shared accept, one issue is that when a new connection becomes acceptable, multiple workers can be notified, creating contention. Please read nginx event loop and describe how nginx tries to resolve the contention.**

Load balancing is performed using multiple worker processes that can all accept on the same listening socket. nginx can enable an accept mutext

```bash
Syntax: accept_mutex on | off;
Default:  
accept_mutex off;
Context:  events
```

which, if enabled, will allow worker processes to accept new connections by turn. If it is toggled off, all worker processes will be notified by new connections and if the volume is low than they might waste resources. The OS will notify all workers whenever an event has occured and the listening socket is readible, so many will try all at once to wake up and content for the accept(). nginx handles this by offering the accept_mutex option, which when enabled, allows one worker at a time to hold the mutex and thus only that worker is allowed to call accept() on listening sockets. The others will have to wait and periodically try to acquire it. Under /event/ngx_event.c, trylock_accept_mutex is used to enable or disable listening socket read events depending on who owns the mutex at the time of call:

```c
if (ngx_use_accept_mutex) {
  if (ngx_accept_disabled > 0) {
      ngx_accept_disabled--;

  } else {
      if (ngx_trylock_accept_mutex(cycle) == NGX_ERROR) {
          return;
      }

      if (ngx_accept_mutex_held) {
          flags |= NGX_POST_EVENTS;

      } else {
          if (timer == NGX_TIMER_INFINITE
              || timer > ngx_accept_mutex_delay)
          {
              timer = ngx_accept_mutex_delay;
          }
      }
  }
}
```

**b. The nginx event loop processes both io events and timers. If it were nginx, how would you implement the 3-second timeout requirement of this project?**

Each nginx event has a timer lingage built into a red black tree of timers and every time a timer is added, ngx_add_timer schedules the event to fire after a specified number of milliseconds. the Main event loop determines the closest timer and passes that through as the timeout to ngx_process_events, then after returning runs the expired timer handlers. To implement a 3 second timeout for this assignment, I would create an event object associated with the connection I wanted to timeout, then set its handler to the specified timeout callback. Then when I begin to wait for the connection, I would schedule the timer and when the condition comples earlier than the timeout, I would simply cancel it.

```rust
/*
Rust psuedocode sketch
*/
struct Event {
  data: ConnectionID,
  handler: fn(&mut EventLoop, ConnectionID),
  expires_ms: u64, 
}
struct EventLoop {
  timers: BinaryHeap<(u64, Event)>, //min-heap expires_ms
  // also need I/O poller, connection table, etc
}
impl EventLoop {
  fn add_timer(&mut self, conn: ConnId, delay_ms: u64, handler: fn(&mut EventLoop, ConnId)) { 
    let now = now_ms(); 
    let mut ev = Event { 
      data: conn, 
      handler, 
      expires_at_ms: now + delay_ms, 
    }; 
    self.timers.push((ev.expires_at_ms, ev)); 
  }
  fn cancel_timer(&mut self, conn: ConnId) { 
    mark_timer_cancelled(conn); 
  }
  fn run(&mut self) {
    loop {
      let now = now_ms();
      let timeout_ms = if let Some((expires_at, _)) = self.timers.peek() {
      if *expires_at <= now { 0 } else { *expires_at - now }
      } else {
        DEFAULT_POLL_TIMEOUT_MS
      };
      let io_events = poll_io(timeout_ms);
      self.handle_io(io_events);
      let now = now_ms();
      while let Some((expires_at, mut ev)) = self.timers.peek().cloned() {
        if expires_at > now {
          break;
        }
        self.timers.pop()
        if is_timer_cancelled(ev.data) {
          continue;
        }
        (ev.handler)(self, ev.data) 
}
fn install_3s_timeout(loop_: &mut EventLoop, conn: ConnId) {
  loop_.add_timer(conn, 3000, timeout_handler);
}
fn timeout_handler(loop_: &mut EventLoop, conn: ConnId) {
  close_connection(loop_, conn);
}
```

**c. nginx processes HTTP in 11 phases. What are the phases? Please list the checker functions of each phase.**

nginx organizes request handling into 11 phases. The phases are as follows, with associated cheker functions

1. NGX_HTTP_POST_READ_PHASE --> associated checker: ngx_http_core_post_read_phase
2. NGX_HTTP_SERVER_REWRITE_PHASE --> associated checker: ngx_http_core_rewrite_phase
3. NGX_HTTP_FIND_CONFIG_PHASE --> associated checker: ngx_http_core_find_config_phase
4. NGX_HTTP_REWRITE_PHASE --> associated checker: ngx_http_core_rewrite_phase
5. NGX_HTTP_POST_REWRITE_PHASE --> associated checker: ngx_http_core_post_rewrite_phase
6. NGX_HTTP_PREACCESS_PHASE --> associated checker: ngx_http_core_generic_phase
7. ACCESNGX_HTTP_ACCESS_PHASES --> associated checker: ngx_http_core_access_phase
8. NGX_HTTP_POST_ACCESS_PHASE --> associated checker: ngx_http_core_post_access_phase
9. NGX_HTTP_TRY_FILES_PHASE --> associated checker: ngx_http_core_try_files_phase
10. NGX_HTTP_CONTENT_PHASE --> associated checker: ngx_http_core_content_phase
11. NGX_HTTP_LOG_PHASE --> associated checker: ngx_http_core_log_phase

**d. A main design feature of nginx is efficient support of upstream; that is, forward request to an upstream server. Can you describe the high level design?**

Upstream support is designed around the ngx_http_upstream_t structure
Upstream server groups are defined with the upstream block and references by directives like proxy_pass, fastcgi_pass, uwsgi_pass, etc. This configuration builds data structures which describe upstream peers, load-balancing, queues, etc. The kep high level design of upstream support involves calling a peer selection method (round robin, hash, random, etc) to choose a backend server from the upstream group. It then establishes a nonblocking connection (ngx_event_connect_peer) and establishes read/write events and timers on the upstream connection. Additionally, upstream i/o is fully asynchronous, and thus the upstream state machine uses two main event handlers on the upstream connection. One is used for reading into buffers, the other for writing a request into the bufers. When data arrives it is read into buffers and passed thru the HTTP output filter chain back to the client connection. Upstream responses are buffered in either memory or on disk. The same filter chain is used for local content and upstream responses. THe upstream core tracks timeouts, errors, and number of tries. On failure it will pick a suitable peer and rerun the connection path. This deisn is effective bdcause it isolates upstream machinery from specific protocols.

**e. nginx introduces a buffer type ngx_buf_t. Please briefly compare ngx_buf_t vs ByteBuffer we covered for Java nio?**

ngx_buf_t is nginx buffer for I/O and differs from ByteBuffer within Java nio in the following ways:

1. ngx_buf_t does not own the storage, rather it points into memory that is typically allocated from ngx_pool_t. Lifetime is tied to the surrouding pool. In contrast the bytebuffer object owns its underlying memory region and the garbace collector controls its lifetime.
2. ngx_buf_t can desribe inmemory buffers and filebacked regions, wheras bytebuffer always represents a mmeory region. In bytebuffer, file data is brought into memory via channels.

___
