# tailsrv

tailsrv watches a single file and streams its contents to multiple clients as it grows.
It's like `tail -f`, but as a server.

* When a client connects, tailsrv sends it data from the file.
* If there is no new data to send, tailsrv waits until the file grows.
* If the socket is full, tailsrv waits for the client to consume some data before sending more.
* Clients can specify an initial byte-offset when they connect.

Some implementation details:

* All data is sent using sendfile.  This means that data is sent by the kernel
  directly from the pagecache to the network card.  No data is ever copied
  into userspace.  This gives tailsrv really good throughput.  However,
  it also means that tailsrv will only run on Linux.
* We use inotify to track modifications to the file.  This means that if the
  file is not growing (and no new clients are connecting) tailsrv does no work.
* We spawn one thread per client.  This means that a slow client can recieve
  data at its own pace, without affecting other clients.

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

### 1. The client sends a header to tailsrv

The header is just an integer, in ASCII, terminated with a newline.  If the
integer is positive, it represents the initial byte offset.  If the integer
is negative, it is interpreted as meaning "counting back from the end of
the file".  Examples:

* `0\n` - start from the beginning of the file
* `1000\n` - start from byte 1000
* `-1000\n` - send the last 1000 bytes

### 2. tailsrv sends data to the client

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


## Non-features

tailsrv opts for extremely simple interfaces for both producers and clients; it
also makes operations very simple for users who don't have complicated
requirements.  It therefore lacks some features you might expect, if you're
coming from eg. Kafka.

* **Multiple files**: Clients can't request data from specific files: it's
  strictly one-file-per-server.  If you want to stream multiple files,
  run multiple instances.
* **Fault tolerance**:  If you need writen data to remain available when your
  fileserver dies, just replicate it to another machine which is also running
  tailsrv.  (If you have two machines physically next to each other, how about
  using DRBD?)  If the seriver dies, so too will clients' connections.  So long
  as they're keeping track of their position in the log, they can connect to
  the backup server and carry on.  Of course, this means clients need to be
  aware of the backup server... sorry!
* **Auto-rotation**: Suppose you're running tailsrv on a file that gets
  rotated.  If the file is moved then tailsrv will exit.  If it's truncated
  then tailsrv will keep going, but won't send clients any more data (until
  the file exceeds its previous length).  Either way, it doesn't work well.
* **Encryption**:  If you can, [use a VPN][wireguard].  Don't trust me with
  your crypto - just make sure the route to your fileserver is secure.  Want to
  completely prohibit insecure access?  Run tailsrv in a network namespace
  which doesn't contain any non-vpn network interfaces.
* **Authentication**: Using a VPN solves this too (when requirements
  are simple).  Want usernames and ACLs?  Kafka has this feature - use
  that instead?

[wireguard]: https://www.wireguard.com


## tailsrv vs Kafka

Kafka does three things:

1. **Collating** messages from multiple producers into a single stream
2. **Indexing** streams by message number
3. **Broadcasting** streams to consumers

(Actually Kafka does many many things, but these are the main ones.)

tailsrv only handles broadcasting.  If you need collating or indexing
functionality, you should roll some software for that yourself and run it
alongside tailsrv on the fileserver.

### Horizontal scalability

Kafka is designed to handle throughputs which would be too much for a
single fileserver.  If you're in that kind of situation, then I'm sorry!
Kafka may help ease the pain a little.

If you _can_ handle everything with a single node though, you're in luck!
You can get away with tailsrv, which is about a million times easier to
set up and manage.

### Collating

Kafka provides an API for reading streams, and also an API for writing to
them.  tailsrv only does the reading side: it doesn't help you coalesce data.
For this you'll need to roll your own solution.

If your data comes from a single process on a single machine, it's dead easy:
you just need to get the data over to the fileserver somehow.  If your data
comes from multiple sources and needs to be carefully aggregated into a single
stream, then you'll need to run another piece of software on fileserver
which accepts connections from your producers and writes the collated data
into a file.

### Indexing

Kafka's abstraction is "streams of messages", whereas tailsrv's abstraction is
"a stream of bytes".  If you want to chop your stream up into messages, just
do that however you'd like (newline-delimited, length-prefixed, etc. etc.).
However, tailsrv doesn't know about your messages, so can't provide indexing
for them.  This means that, if you want to start reading from a certain
message, you have to know its byte-offset.  You could do this with another
piece of software on the fileserver (an indexer).  Kafka does this for you,
but tailsrv doesn't so you'll have to roll your own.


## Licence

This software is in the public domain.  See UNLICENSE for details.
