# tailsrv

**STATUS:** It mostly works, but some documented features are missing. (Look
out for "TODO"/"FIXME" in the documentation below.)

tailsrv is a high-performance file-streaming server.  It's like `tail -f` in
server form.  It has high throughput, low latency, and scales to lots of
clients (see [Performance](#performance)).  Setup is very simple, and clients
don't require a special library.  It is, however, Linux-only (see
[Limitations](#limitations)).

Client connections are monitored with epoll.  Files are monitored with inotify.
When a file changes or a connection becomes writable, tailsrv `sendfile()`s the
new data.

Compared to a simple TCP connection between your producer and consumer, a
tailsrv instance in the middle can be used to provide:

* **fan-in**:  many producers, safely interleaved.
* **fan-out**:  many consumers, without putting extra stress on the producer.
* **replay**:  go back in time though a socket's history.

For a quick-start, see the [example usage](#example).


## Usage

```
tailsrv 0.3
A server which allows clients to tail files in the working directory

USAGE:
    tailsrv [FLAGS] --port <port>

FLAGS:
    -h, --help       Prints help information
    -i, --index      Lazily maintain index files in /tmp for faster seeking
    -q, --quiet      Don't produce output unless there's a problem
    -V, --version    Prints version information

OPTIONS:
    -p, --port <port>    The port number on which to listen for new connections
```

When tailsrv is started, by default all regular files in and below the working
directory become available for streaming.  You can exclude files by writing
globs in a ".ignore" file - the syntax is the same as ".gitignore".  Excluded
files will not show up in listings, and will not be streamable.


## Protocol

Clients open a TCP connection, send a UTF-8-encoded newline-terminated header,
and then start reading data, hanging up when they're done.  The grammar for the
headers is:

```
HEADER := list [dir] | stream <file> [from <INDEX>]
INDEX  := start | end | byte <n> | line <n> | seqnum <n>
```

`dir` and `file` are paths which may optionally begin with a '/' character
(TODO).

Fields labeled as `<n>` are parsed as signed integers.  If the value is
negative, it is interpreted as meaning "counting back from the end of the file"
(TODO).

If the watched file is deleted or moved, tailsrv will terminate the connection.
This is the only (non-error) condition in which tailsrv will end a stream.

At all times, communication is one-way: first the client sends a header, then
tailsrv sends some data.  tailsrv will send nothing until a newline is
recieved, and once a newline has been recieved it will ignore anything sent by
the client.


## Indexing

tailsrv allows you to specify the point in the log at which it will start
streaming data.  This position may be specified according to the following
indexing schemes:

* **byte offset**:  `stream <file> from byte <n>`. Does what it says on the tin.
  If the offset is greater than the length of the file, tailsrv waits until the
  file reaches to desired length before sending any data.
* **line number**:  `stream <file> from line <n>`.  If the file contains fewer
  than `<n>` lines, tailsrv just starts streaming from the end (FIXME).  *This
  indexing strategy only makes sense with files containing newline-delimited
  data.*
* **sequence number**:  `stream <file> from seqnum <n>`.  If the seqnum
  is greater than the number of blobs in the file, tailsrv just starts
  streaming from the end (FIXME).  *This indexing strategy only makes sense
  with files which are a concatenation of length-prefixed blobs, where the
  length is encoded as a [base128 varint].* Note: you need to compile with
  the "prefixed" feature enabled.

[base128 varint]: https://developers.google.com/protocol-buffers/docs/encoding#varints

If the `-i` flag is passed to tailsrv, it will maintain index files in /tmp -
one for each log file and indexing method (TODO).  These indexes are built and
updated lazily, so that unfollowed files don't incur a cost.  They can speed up
seeks dramatically for large files.  Be aware though: if you use this option,
you *really* need to make sure your log files are append-only!  If not, seeking
will be totally broken.


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

tailsrv is Linux-only, due to its explicit dependence on epoll, inotify, and
sendfile.  This should be fixable, with some effort.

tailsrv uses an inotify watch for each file.  This puts an upper limit on the
number of watched files: see `/proc/sys/fs/inotify/max_user_watches` (the
default is 64k).  If two clients watch the same file, only one watch is used.
When all clients for a file disconnect, the watch is removed.

The server operator must ensure that all watched files are append-only.
tailsrv won't crash if you modify the middle of a file, but any expectations
about log replayability your clients may have will be broken.


## Non-features and missing features

tailsrv opts for extremely simple interfaces for both producers and clients; it
also makes operations very simple for users who don't have complicated
requirements.  It therefore lacks some features you might expect, if you're
coming from eg. Kafka.

Non-features:

* **In-band session control**:  A client can't communicate with tailsrv one it
  has begun sending data.  Therefore, if you need to seek to a different point
  in the log, you must hang up and start a new connection.
* **File multiplexing**:  You can't multiplex data from multiple files across
  the same TCP connection: it's strictly one-file-per-connection.  If you want
  to stream multiple files, open multiple connections.
* **Fault tolerance**:  If you need writen data to remain available when your
  log server dies, just replicate it to another machine which is also running
  tailsrv.  (If you have two machines physically next to each other, how about
  using DRBD?)  If the seriver dies, so too will clients' connections.  So long
  as they're keeping track of their position in the log, they can connect to
  the backup server and carry on.  Of course, this means clients need to be
  aware of the backup server... sorry!
* **Auto-rotation**:  Rotation must be implemented manually, and the producer
  and consumer must agree on the policy.  For instance, suppose the producer
  increments a counter in the filename after 1GB.  Then the consumer must keep
  track of how many bytes it has read, and start reading the next file when
  it reaches the 1GB mark.
* **Encryption**:  If you can, [use a VPN][wireguard].  Don't trust me with
  your crypto - just make sure the route to your fileserver is secure.  Want to
  completely prohibit insecure access?  Run tailsrv in a network namespace
  which doesn't contain any non-vpn network interfaces.
* **Authentication**:  Using a VPN solves this too (when requirements are
  simple).
* **Compression**:  You can compress your messages when you write them, but
  there's a conflict between compression efficiency and fine-grained indexing.
  Perhaps you can regain some efficiency by [preparing a dictionary][zstd]?

[wireguard]: https://www.wireguard.com
[zstd]: https://github.com/facebook/zstd#the-case-for-small-data-compression

Limitations of the design:

* **Good compression**:  tailsrv doesn't do any on-the-fly compression,
  and it won't index into a compressed chunk.  This means that it can't send
  data which is compressed in large chunks but indexable in small chunks.
  Kafka has a decent story for this, however - consider using that instead?
* **Ad-hoc encryption**:  What if you don't have a VPN?  You're going to have
  to encrypt/decrypt messages client-side.  This is not a great situation (good
  luck achieving eg. forward secrecy).  Ideally tailsrv should support
  encrypted sockets (TLS or noise sockets).
* **Fancy authentication**:  So you want usernames and ACLs, huh?  Sorry, we
  don't do those.  Kafka has this feature though - use that instead?
* **Exotic indexing**:  "Send me data starting at 9am this morning."  Your log
  data may contain timestamps, but tailsrv doesn't know about them.  There's no
  way to extend tailsrv with custom indexing methods (at present).

<!--
The fundamental difference between tailsrv and Kafka is how indexing works:  in
tailsrv, you're referring to properties of the underlying storage (byte offset,
line number, etc.); in Kafka, you're referring to an abstract "message number".

tailsrv forces the user to decide on a number of trade-offs - tensions arising
from the fact that indexing and storage are not separated:

1. Log rotation is visible. Producers and consumers must be aware of the
   rotation policy.
2. If you do large-scale compression, you lose index granularity.

The reason Kafka abstracts indexing away from storage is so that you can have
your cake and eat it, with regard to the above trade-offs.

So Kafka presents an opaque abstract interface: and all details of the
underlying storage are hidden, and all access to your data must happen via the
broker.  tailsrv, by contract, is transparent, and can therefore fuse the
producer interface and the underlying storage.  However, Kafka's abstraction
enables some key features, which tailsrv necessarily lacks.
-->


## Producing data

Perhaps you want to write all your log-structured data to one place, and then
consume it elsewhere?  This Kafka-style approach to data processing has become
popular lately, and tailsrv can function as a component in such a setup.

tailsrv will allow consumers to connect to your log server, but it doesn't help
you get data onto it in the first place - for this task you'll need to use
something else.  Here are some ideas:

* For data which should be streamed with low latency, how about `producerprog |
  ssh logserver "cat >> logfile"`?
* For data which can be written in batches, how about writing it locally and
  then periodically running `rsync --append`?
* If you're happy to invert the direction of control, how about running `nc
  producerserver 5432 >> logfile` on the logserver?


<!--
I'm considering a way
to register "filters" with tailsrv which will extend its functionality. A
"filter" is a program which takes a filepath and any other data the client
provides in the header, and which returns a byte-offset into the given file. An
example:

```
$ tsindex foo.log 2017-08-12 19:29
2114893
$ tailsrv -p 4321 -f 'timestamp tsindex'
INFO:tailsrv: Serving files from "/var/log" on 0.0.0.0:4321
INFO:tailsrv: Registered filter "tsindex" as "timestamp"
$ echo "stream foo.log from timestamp 2017-08-12 19:29" | nc logserver 4321
```
-->


## Example

Pick a port number and start the server in your desired directory:

```
logserver:/var/log$ tailsrv -p 4321
INFO:tailsrv: Serving files from "/var/log" on 0.0.0.0:4321
```

Now that tailsrv is running in logserver:/var/log, the following commands will
do roughly the same thing:

```
$ ssh logserver -- tail -f -n+1000 /var/log/nginx/access.log
$ echo "stream nginx/access.log from line 1000" | nc logserver 4321
```

Rather than using netcat, however, you probably want to connect to tailsrv
directly from your log-consuming application. This is very easy:

```rust
const log: &str = "nginx/access.log";
const offset: usize = 1000;

let sock = TcpStream::connect("logserver:4321")?;
writeln!(sock, "stream {} from line {}", log, offset)?;
for line in BufReader::new(sock).lines() {
    /* handle log data */
}
```

The example above is written in rust, but you can connect to tailsrv from any
programming language without the need for a special client library.


<!--
Server:

```
# ip link add dev wg0 type wireguard
# ip address add dev wg0 10.0.0.1/24
# wg setconf wg0 myconfig.conf
# ip netns add tailsrv
# ip link set wg0 netns tailsrv
$ ip netns exec tailsrv tailsrv -p 4321
```

Client:

```
# ip link add dev wg0 type wireguard
# ip address add dev wg0 10.0.0.2/24
# wg setconf wg0 myconfig.conf
$ nc
```
-->

Licence
-------

This software is in the public domain.  See UNLICENSE for details.
