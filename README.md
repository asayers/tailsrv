<h1 align="center">tailsrv</h1>

tailsrv watches a single file and streams its contents to multiple clients as it grows.
It's like `tail -f`, but as a server.

* When a client connects, tailsrv sends it the current contents of the file.
* When the file grows, tailsrv sends the new data to all clients.
* Clients can specify a byte-offset when they connect; tailsrv will not send
  data before that position in the file.
* If a client's socket is full, tailsrv waits for the client to consume some
  data before sending more.  Other clients are not affected.

tailsrv is low-latency, high-throughput, and consumes minimal system resources.
It requires Linux >=5.7.

Some implementation details:

* All data goes directly from the pagecache to the network card, never copied
  into userspace. This is done using the `splice()` syscall (effectively we're
  doing `sendfile()`, but I have to emulate it since io_uring doesn't support
  sendfile yet.)
* We use inotify to track modifications to the file.  This means that, when
  things are calm, the tailsrv process can go to sleep, and will be woken up by
  the kernel when the file grows (or a new client connects).
* The I/O is dispatched using io_uring.  This means that the number of threads
  required doesn't depend on the number of clients.  Thousands of clients can
  connect simulateneously without slowing down the system.

If you're interested in how tailsrv compares to Kafka, see [here](vs_kafka.md)
for a comparison.


## Usage example

Let's say you have a machine called `webserver`.  Pick a port number and
start tailsrv:

```console
$ tailsrv -p 4321 /var/log/nginx/access.log
```

tailsrv is now watching access.log.  You can connect to tailsrv from your
laptop and stream the contents of the file:

```console
$ echo "1000" | nc webserver 4321
```

You will immediately see the contents of access.log, starting from byte 1000,
up to the end of the file.  The connection remains open, waiting for new data.
As soon as nginx writes a line to access.log, it will appear on your laptop.
It's more-or-less the same as if you did this:

```console
$ ssh webserver -- tail -f -c+1000 /var/log/nginx/access.log
```

Rather than using netcat, however, you probably want to connect to tailsrv
directly from your log-consuming application.

```rust
let sock = TcpStream::connect("webserver:4321")?;
writeln!(sock, "{}", 1000)?;
for line in BufReader::new(sock).lines() {
    /* handle log data */
}
```

The example above is written in rust, but as you can see it's very
straightforward: you can to do this from any programming language without
the need for a special client library.


## Protocol

### Step 1: the client sends a header to tailsrv

The header is just an integer, in ASCII, terminated with a newline.  If the
integer is positive, it represents the initial byte offset.  If the integer
is negative, it is interpreted as meaning "counting back from the end of
the file".  Examples:

* `0\n` - start from the beginning of the file
* `1000\n` - start from byte 1000
* `-1000\n` - send the last 1000 bytes

### Step 2: tailsrv sends data to the client

Once it receives a header, tailsrv will start sending you file data.

...and that's it as far as the protocol goes.
tailsrv will ignore everything you send to it after the newline.
When you're done, just close the connection.
tailsrv will not terminate the connection unless it is shutting down.

There's no in-band session control: if you want to seek to a different
position in the file, close the connection and open a new one.

### The file

tailsrv expects a file which will be appended to.  If the watched file is
deleted or moved, tailsrv will exit.  If you modify the middle of the file -
well, nothing disasterous will happen, but your clients might get confused.

## Features

### tracing-journald

Enables a dependency on
[tracing-journald](https://crates.io/crates/tracing-journald) crate and adds a
new `--journald` command-line flag. This will redirect all the tracing output to
the system `journald` which gives much richer information than the default
output formatter. Especially useful if you're planning to run `tailsrv` as a
systemd service.

### sd-notify

Enables a dependency on [sd-notify](https://crates.io/crates/sd-notify) crate.
`tailsrv` is going to send a systemd readiness notification once it starts
accepting connections from clients. This is useful combined with a `notify`
systemd service type.

## Licence

This software is in the public domain.  See UNLICENSE for details.
