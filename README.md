**Status:** in-development. Don't use this yet.

# tailsrv

tailsrv is `tail -f` in server form. Don't write

```
ssh logserver -- tail -f -c+1000 /var/log/nginx/access.log
```

...instead write:

```
echo "stream nginx/access.log from byte 1000" | nc logserver 4321
```

...or something like:

```rust
const log: &str = "nginx/access.log";
const offset: usize = 1000;

let sock = TcpStream::connect("logserver:4321")?;
writeln!(sock, "stream {} from byte {}", log, offset)?;
for line in BufReader::new(sock).lines() {
    /* handle log data */
}
```

tailsrv is a simple file-streaming server. It's high-performance (see
[Performance characteristics](#performance-characteristics)) and scales to lots
of clients. It is, however, Linux-only.

## Usage

### To start the server:

```
logserver:/var/log$ tailsrv -p 4321
INFO:tailsrv: Serving files from "/var/log" on 0.0.0.0:4321
```

All regular files under /var/log are now available for streaming, except those
which are ignored. You can ignore files by writing globs in a ".ignore" file -
the syntax is the same as ".gitignore".

### Listing available files

```
logserver:~$ echo "list" | nc logserver 4321
auth.log
boot.log
bootstrap.log
dmesg
kern.log
mail.err
mail.log
nginx/access.log
nginx/error.log
syslog
Xorg.log
```

### Recieving any new data appended to "syslog"

```
logserver:~$ echo "stream syslog" | nc logserver 4321
Aug 10 11:16:05 logserver kernel: [3103663.292000] usb 3-3: new high-speed USB device number 89 using xhci_hcd
Aug 10 11:16:05 logserver kernel: [3103663.420080] usb 3-3: New USB device found, idVendor=0424, idProduct=2514
Aug 10 11:16:05 logserver kernel: [3103663.420082] usb 3-3: New USB device strings: Mfr=0, Product=0, SerialNumber=0
^C
```

This starts streaming from the current end-of-file; ie. the client will recieve
any data appended to "syslog" after establishing the connection, but none of
the existing data.

If "syslog" is deleted or moved, tailsrv will terminate the connection. This is
the only (non-error) condition in which tailsrv will end a stream.

### Replaying "syslog" from a specific point

```
logserver:~$ echo "stream syslog from byte 1000" | nc logserver 4321
ctivating via systemd: service name='org.freedesktop.hostname1'
Aug 10 08:50:47 logserver systemd[1]: Starting Hostname Service...
Aug 10 08:50:47 logserver systemd-udevd[305]: specified group 'admin' unknown
^C
```

tailsrv will send everything from byte 1000 to the end of file, and then
continue streaming new data.

If "syslog" were less than 1000 bytes long, tailsrv would have waited until it
reached 1000 bytes and then started streaming.

### Streaming "syslog", including a certain amount of replayed data

```
logserver:~$ echo "stream syslog from byte -200" | nc logserver 4321
sg1 type 0
Aug 10 11:16:07 logserver kernel: [3103665.233587] sd 34:0:0:1: Attached scsi generic sg2 type 0
Aug 10 11:16:07 logserver kernel: [3103665.246092] sd 34:0:0:0: [sdb] Attached SCSI removable disk
^C
```

tailsrv will send the last 200 bytes from the file, and then continue streaming
new data.

If "syslog" were less than 200 bytes long, tailsrc would send the whole file
and then continue streaming new data.

### Custom clients

Of course, you don't have to use netcat to interact with tailsrv. Just open a
TCP connection, send a header in plain text, and start recieving data. Read as
fast or as slow as you like. When you're done just hang up.

```rust
// An example client written in rust
let sock = TcpStream::connect("127.0.0.1:4321")?;
writeln!(sock, "stream nginx/access.log")?;
for line in BufReader::new(sock).lines() {
    println!("Someone accessed nginx!");
}
println!("nginx access log was deleted or moved.");
```

tailsrv ignores the client after the initial header is recieved. Once a
connection has data coming through it, the client can't control the session in
any way. Specifically:

- Streams are not seekable. If you need to seek to a different point in the log
  file, hang up and start a new connection.
- There's no multiplexing. If you want to stream multiple files, open multiple
  connections.

This design is deliberate and there's no plan to change it.

## Performance characteristics

tailsrv is a simple program. Clients open a TCP connection and send a header.
Connections are monitored with epoll. Files are monitored with inotify. When a
file has changed and a connection is writable, we `sendfile()` the new data.

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

> Seeking to a byte-offset is all well and good, but I want to stream starting
> at line 103! Now, open a stream starting at the first line written today -
> the timestamps are all in there!

Unlike failover, it's *not* possible to address the indexing problem at the
client side. And unlike producing, this is directly related to the job of
tailsrv. This sounds like a feature tailsrv should have!

Unfortunately, there are so many ways clients might want to index their data.

- Perhaps we want to skip the nth occurance of some delimiting byte-sequence
  (such as `\n`)? For reasonably-sized log files, it's probably acceptable to
  compute this on the fly.
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
$ cat linebyte.sh
grep -b -m$2 '' $1 | tail -1 | sed s/:.*//
$ tailsrv -p 4321 -f 'line=./linebyte.sh'
INFO:tailsrv: Serving files from "/var/log" on 0.0.0.0:4321
INFO:tailsrv: Registered filter "./linebyte.sh" as "line"
$ echo "stream foo.log from line 15" | nc logserver 4321
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
