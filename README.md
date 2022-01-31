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

Let's say the machine is called `logserver`.  Pick a port number and start
tailsrv:

```console
$ tailsrv -p 4321 /var/log/nginx/access.log
```

Now that tailsrv is running, the following commands will do roughly the
same thing:

```console
$ ssh logserver -- tail -f -n+1000 /var/log/nginx/access.log
$ echo "1000" | nc logserver 4321
```

Rather than using netcat, however, you probably want to connect to tailsrv
directly from your log-consuming application. This is very easy:

```rust
let sock = TcpStream::connect("logserver:4321")?;
writeln!(sock, "{}", 1000)?;
for line in BufReader::new(sock).lines() {
    /* handle log data */
}
```

The example above is written in rust, but you can connect to tailsrv from any
programming language without the need for a special client library.


## Performance

We use inotify to track modifications to files.  This allows us to avoid the
latency associated with polling.  It also means that watches of quiescent files
don't have any performance cost.

We use epoll to track whether clients are writable.  This means that a slow
client can recieve data at its own pace, but it won't block other clients (even
though tailsrv uses only a single thread for sending data).

The use of sendfile means that *all data* is sent by the kernel directly from
the pagecache to the network card.  No data is ever copied into userspace.
This gives tailsrv really good throughput.

Clients can read data as fast or as slow as they please, without affecting each
other.  Some fairness properties are guaranteed (TODO: document these).

TODO: Benchmarks


## Limitations

tailsrv is Linux-only, due to its use of sendfile.  I plan to add some
fallback code and make it cross platform, however.

tailsrv uses an inotify watch for each file.  This puts an upper limit on
the number of watched files: see `/proc/sys/fs/inotify/max_user_watches`
(the default is 64k).  If two clients watch the same file, only one watch
is used.  When all clients for a file disconnect, the watch is removed.

It's not a hard requirement, but your clients probably expect the watched
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
  log server dies, just replicate it to another machine which is also running
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


## Using tailsrv as a Kafka alternative

Perhaps you want to write all your log-structured data to one place, and then
consume it elsewhere?  This Kafka-style approach to data processing has become
popular lately, and tailsrv can function as a component in such a setup.

tailsrv will allow consumers to connect to your log server, but it doesn't help
you get data onto it in the first place - for this task you'll need to use
something else.  Here are some ideas:

* For data which should be streamed with low latency, how about
  `producerprog | ssh logserver "cat >> logfile"`?
* For data which can be written in batches, how about writing it locally and
  then periodically running `rsync --append`?
* If you're happy to invert the direction of control, how about running
  `nc producerserver 5432 >> logfile` on the logserver?


## Licence

This software is in the public domain.  See UNLICENSE for details.
