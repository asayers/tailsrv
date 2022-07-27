# tailsrv

tailsrv is a high-performance file-streaming server.  It's like `tail -f` in
server form.  It has high throughput, low latency, and scales to lots of
clients (see [Performance](#performance)).  Setup is very simple, and clients
don't require a special library.  It is, however, Linux-only (see
[Limitations](#limitations)).

Here's how it works in a nutshell:

* When a client connects, it gives an initial byte offset to start.
* tailsrv sends it data from the file until the client is full or up-to-date.
* When the client becomes writable, it is sent more data.
* When the file is appended to, the new data is sent to the clients.

Compared to a simple TCP connection between your producer and consumer, a
tailsrv instance in the middle can be used to provide:

* **fan-in**: many producers can write to the same file.  (But take care with interleaving!)
* **fan-out**:  many consumers can read the producer's output without stressing it.
* **replay**:  go back in time though a socket's history.

For a quick-start, see the [example usage](#example).


## Usage

Clients open a TCP connection and send a header.  This header should be
a single signed integer, formatted as a decimal string, UTF8-encoded, and
terminated with a newline.  The integer represents the byte offset at which
to start.  If the value is negative, it is interpreted as meaning "counting
back from the end of the file".  Examples:

* `0\n` - start from the beginning of the file
* `1000\n` - start from byte 1000
* `-1000\n` - send the last 1000 bytes

After sending a header, the client should start reading data from the socket.
At all times, communication is one-way: first the client sends a header,
then tailsrv sends some data.  tailsrv will send nothing until a newline is
recieved, and once a newline has been recieved it will ignore anything sent
by the client.  The client may unceremoniously hang up at any time.

tailsrv will not terminate the connection for any reason, unless it is
shutting down.  If the watched file is deleted or moved, tailsrv will exit.


### Example

Let's say you have a machine called `webserver`.  Pick a port number and
start tailsrv:

```console
$ tailsrv -p 4321 /var/log/nginx/access.log
```

tailsrv is now watching access.log.  You can now connect to it from your
laptop and stream the contents of the file:

```console
$ echo "1000" | nc webserver 4321
```

You should see access.log, starting at byte 1000, and it will stream new
contents as they come in.  It's more-or-less the same as if you did this:

```console
$ ssh webserver -- tail -f -c+1000 /var/log/nginx/access.log
```

Rather than using netcat, however, you probably want to connect to tailsrv
directly from your log-consuming application. This is very easy:

```rust
let sock = TcpStream::connect("webserver:4321")?;
writeln!(sock, "{}", 1000)?;
for line in BufReader::new(sock).lines() {
    /* handle log data */
}
```

The example above is written in rust, but as you can see very straightforward.
You can to do this from any programming language without the need for a
special client library.


## Performance

* We use inotify to track modifications to the file.  This means that if the
  file is not growing (and no new clients are connecting) tailsrv does no work.
* We spawn one thread per client.  This means that a slow client can recieve
  data at its own pace, without affecting other clients.
* All data is sent using sendfile.  This means that data is sent by the kernel
  directly from the pagecache to the network card.  No data is ever copied
  into userspace.  This gives tailsrv really good throughput.

TODO: Benchmarks


## Limitations

* tailsrv is Linux-only, due to its use of sendfile.  I plan to add some
  fallback code and make it cross platform, however.
* It's not a hard requirement, but your clients probably expect the watched
  file to be append-only and tailsrv won't do anything to enforce that.


## Non-features

tailsrv opts for extremely simple interfaces for both producers and clients; it
also makes operations very simple for users who don't have complicated
requirements.  It therefore lacks some features you might expect, if you're
coming from eg. Kafka.

* **In-band session control**:  A client can't communicate with tailsrv one it
  has begun sending data.  Therefore, if you need to seek to a different point
  in the log, you must hang up and start a new connection.
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

Kafka does something very similar to tailsrv, but is designed to handle
throughputs which would be too much for a single fileserver.  If you're in
that kind of situation, then I'm sorry!  You're about to give up a lot of
nice things in the name of horizontal scalability.  Kafka can help ease the
pain a little.

If you _can_ handle everything with a single node though, you're in luck!
You can get away with tailsrv, which is about a million times easier to
set up and manage.

### Fan-in

Kafka provides an API for reading streams, and also an API for writing to
them.  tailsrv only does the reading side: it doesn't help you coalesce data.
For this you'll need to roll your own solution.

If your data comes from a single process on a single machine, it's dead easy:
you just need to get the data over to the fileserver somehow.  If your data
comes from multiple sources and needs to be carefully aggregated into a single
stream, then you'll need to run another piece of software on fileserver
which accepts connections from your producers and writes the collated data
into a file.


## Licence

This software is in the public domain.  See UNLICENSE for details.
