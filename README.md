# tailsrv

tailsrv is a high-performance file-streaming server. It's like `tail -f` in
server form. It has high throughput, low latency, and scales to lots of clients
(see [Performance characteristics](#performance-characteristics)). It is,
however, Linux-only (see [Limitations](#limitations)).

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

tailsrv is a relatively simple single-threaded program. Client connections are
monitored with epoll. Files are monitored with inotify. When a file changes or
a connection becomes writable, we `sendfile()` the new data.

## Usage

```
tailsrv 0.2
A server which allows clients to tail files in the working directory

USAGE:
    tailsrv [FLAGS] --port <port>

FLAGS:
    -h, --help       Prints help information
    -q, --quiet      Don't produce output unless there's a problem
    -V, --version    Prints version information

OPTIONS:
    -p, --port <port>    The port number on which to listen for new connections
```

When tailsrv is started, by default all regular files in and below the working
directory become available for streaming. You can exclude files by writing
globs in a ".ignore" file - the syntax is the same as ".gitignore". Excluded
files will not show up in listings, and will not be streamable.

## Protocol

Clients open a TCP connection, send a UTF-8-encoded newline-terminated header,
and then start reading data. Read as fast or as slow as you like. When you're
done just hang up. The grammar for the headers is something like this:

```
HEADER := list [dir] | stream <file> [from <INDEX>]
INDEX  := start | end | byte <n> | line <n>
```

- **paths**: `dir` and `file` are paths which may optionally begin with a '/'
  character (TODO).
- **byte-offsets**: `n` is a signed integer. If it is negative, it is
  interpreted as meaning "counting back from the end of the file" (TODO). If it
  is greater than the current length of the file, tailsrv waits until the file
  reaches to desired length before sending any data.
- **line-offsets**: `n` is a signed integer. If it is negative, it is
  interpreted as meaning "counting back from the end of the file" (TODO). If it
  is greater than the current length of the file, tailsrv just starts streaming
  from the end (FIXME).

Communication happens in two phases: tailsrv will send nothing until a newline
is recieved, and once a newline has been recieved it will ignore anything sent
by the client. This implies that:

- Sessions is not seekable. If you need to seek to a different point in the
  log, hang up and start a new connection.
- There's no multiplexing. If you want to stream multiple files, open multiple
  connections.

This design is deliberate and there's no plan to change it.

If the watched file is deleted or moved, tailsrv will terminate the connection.
This is the only (non-error) condition in which tailsrv will end a stream.

## Performance characteristics

We use inotify to track modifications to files. This allows us to avoid the
latency associated with polling. It also means that watches of quiescent files
don't have any performance cost.

We use epoll to track whether clients are writable. This means that a slow
client can recieve data at its own pace, but it won't block other clients (even
though tailsrv uses only a single thread).

The use of sendfile means that *all data* is sent by the kernel directly from
the pagecache to the network card. No data is ever copied into userspace. This
gives tailsrv really good throughput.

TODO: Benchmarks

## Limitations

The big one: the explicit dependence on epoll, inotify, and sendfile makes
tailsrv Linux-only. Expanding portability to other unixes should be possible,
with some effort.

tailsrv uses an inotify watch for each file. This puts an upper limit on the
number of watched files: see `/proc/sys/fs/inotify/max_user_watches` (the
default is 64k). If two clients watch the same file, only one watch is used.
When all clients for a file disconnect, the watch is removed.

The server operator must ensure that all watched files are append-only. tailsrv
won't crash if you modify the middle of a file, but any expectations about log
replayability your clients may have will be broken.

## Non-features

### Failure tolerance

If you need writen data to remain available when your log server dies, just
replicate it to another machine which is also running tailsrv. If the machines
are next to each other, how about DRBD? We don't see a good reason for this
functionality to be incorporated into tailsrv.

If you want consumers to automatically start reading from the new server...
well I'm afraid you're going to have to implement something yourself. Just make
sure your client always keeps track of where it is in the log, so that when it
connects to the backup server it knows what byte-offset to give.

Automatic failover is a problem which is best addressed on the client-side, so
support for it will not be included in tailsrv. For a lot of clients, it's also
unnecessary.

### Producing data

Perhaps you want to write all your log-structured data to one place, and then
consume it elsewhere? This Kafka-style approach to data processing has become
popular lately, and tailsrv can function as a component in such a setup.

tailsrv will allow consumers to connect to your log server, but it doesn't help
you get data onto it in the first place - for this task you'll need to use
something else. Here are some ideas:

- For data which should be streamed with low latency, how about `producerprog |
  ssh logserver "cat >> logfile"`?
- For data which can be written in batches, how about writing it locally and
  then periodically running `rsync --append`?
- If you're happy to invert the direction of control, how about running netcat
  on the logserver to connect to the producer?  `nc producerserver 5432 >>
  logfile`?

### Indexing

> I want a stream starting at the first line written today - the timestamps are
> all in there!

Unlike failover, it's *not* possible to address the indexing problem at the
client side. And unlike producing, this is directly related to the job of
tailsrv. This sounds like a feature tailsrv should have!

Unfortunately, there are so many ways clients might want to index their data.

- Perhaps we want to skip the nth occurance of some delimiting byte-sequence
  (other than `\n`)? For reasonably-sized log files, it's probably acceptable
  to compute this on the fly.
- Perhaps our log file is a concatenation of length-prefixed binary blobs, and
  we want to seek to the nth blob? If the logs are large, we'll probably need
  to maintain an index for this.
- Perhaps the entries in our log contain some monotonically increasing field,
  and we want to binary-search for the first entry which exceeds a given value?
  Now you're asking tailsrv to parse your log entries...

I sense the danger of feature-creep here, so tailsrv currently has no indexing
support.

However, I'm considering providing a way to register "filters" with tailsrv
which will extend its functionality. A "filter" is a program which takes a
filepath and any other data the client provides in the header, and which
returns a byte-offset into the given file. An example:

```
$ tsindex foo.log 2017-08-12 19:29
2114893
$ tailsrv -p 4321 -f 'timestamp tsindex'
INFO:tailsrv: Serving files from "/var/log" on 0.0.0.0:4321
INFO:tailsrv: Registered filter "tsindex" as "timestamp"
$ echo "stream foo.log from timestamp 2017-08-12 19:29" | nc logserver 4321
```

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
