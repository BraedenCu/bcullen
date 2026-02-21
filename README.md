# CPSC433 Assignment One: Rust HTTP/1.x Protocol and Server

___

## Program Architecture

```text
                          ┌─────────────────────────────────┐
                          │           main.rs                │
                          │  CLI parsing, ServerConfig load  │
                          │  Spawn management terminal thread│
                          └───────────┬─────────────────────┘
                                      │
                     ┌────────────────┴────────────────┐
                     │ ProcessingMode?                  │
                     ▼                                  ▼
          ┌──────────────────┐              ┌─────────────────────┐
          │   server.rs      │              │   select_loop.rs    │
          │  Thread Pool     │              │  Event Loop (mio)   │
          │  (nThreads N)    │              │  (nSelectLoops N)   │
          └──────┬───────────┘              └──────┬──────────────┘
                 │                                 │
                 │  TcpListener::accept()          │  TcpListener::accept()
                 │  (non-blocking)                 │  (non-blocking)
                 │                                 │
                 ▼                                 ▼
     ┌───────────────────────┐       ┌──────────────────────────────┐
     │  mpsc channel dispatch│       │  Round-robin dispatch via    │
     │  to N worker threads  │       │  mpsc channels to N loops    │
     └───────┬───────────────┘       └──────┬───────────────────────┘
             │                              │
             ▼                              ▼
  ┌─────────────────────┐       ┌──────────────────────────────────┐
  │  connection.rs       │      │  Per-connection state machine    │
  │  handle_connection() │      │  ConnState:                      │
  │  (blocking I/O,      │      │    ReadingRequest                │
  │   loop for           │      │    WritingResponse               │
  │   keep-alive)        │      │    Done                          │
  └─────────┬────────────┘      │  mio::Poll for readiness events  │
            │                   └──────┬───────────────────────────┘
            │                          │
            └────────┬─────────────────┘
                     │
                     ▼
    ┌──────────────────────────────────────┐
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
    └──────────────────────────────────────┘

    ┌──────────────────────────────────────┐
    │         Shared State (Arc)           │
    │                                      │
    │  server.rs :: ServerState            │
    │  ├─ config: ServerConfig             │
    │  ├─ shutdown: AtomicBool             │
    │  ├─ accepting: AtomicBool            │
    │  └─ active_connections: AtomicUsize  │
    └──────────────────────────────────────┘

    ┌──────────────────────────────────────┐
    │         Source File Map              │
    │                                      │
    │  main.rs ─────── Entry, CLI, mgmt    │
    │  config.rs ───── Apache-style parse  │
    │  server.rs ───── Thread pool mode    │
    │  select_loop.rs  Event loop mode     │
    │  connection.rs ─ Blocking handler    │
    │  request.rs ──── HTTP request parse  │
    │  response.rs ─── HTTP response build │
    │  router.rs ───── URL routing, auth   │
    │  cgi.rs ──────── CGI execution       │
    │  mime.rs ─────── MIME type lookup    │
    │  util.rs ─────── Date formatting     │
    └──────────────────────────────────────┘
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

With the server running, run these from a separate terminal:

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
___
