**So... we NIH'd Kafka**. TL;DR: If you want to stream log files from a central
location to many consumers, but don't need Kafka's other features, read on for
a low-complexity alternative.

# tailsrv

tailsrv is `tail` in server form. Don't write
```
ssh atala -- tail -f -c+1000 access.log
```

Instead write
```
echo "stream access.log from byte 1000" | nc atala 4321
```

It does the same thing (minus authentication/encryption), and it scales
properly. It is, however, Linux-only.

## Usage

### To start the server:

```
atala:/var/log$ tailsrv -p 4321
INFO:tailsrv: Serving files from "/var/log" on 0.0.0.0:4321
```

All regular files under /var/log are now available for streaming, except those
which are ignored. You can ignore files by writing globs in a ".ignore" file -
the syntax is the same as ".gitignore".

### Listing available files

```
atala:~$ echo "list" | nc atala 4321
auth.log
boot.log
bootstrap.log
chrony/measurements.log
chrony/statistics.log
chrony/tracking.log
cups/access_log
cups/error_log
cups/page_log
dmesg
faillog
fsck/checkfs
fsck/checkroot
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
atala:~$ echo "stream syslog" | nc atala 4321
Aug 10 11:16:05 atala kernel: [3103663.292000] usb 3-3: new high-speed USB device number 89 using xhci_hcd
Aug 10 11:16:05 atala kernel: [3103663.420080] usb 3-3: New USB device found, idVendor=0424, idProduct=2514
Aug 10 11:16:05 atala kernel: [3103663.420082] usb 3-3: New USB device strings: Mfr=0, Product=0, SerialNumber=0
Aug 10 11:16:05 atala kernel: [3103663.420826] hub 3-3:1.0: USB hub found
Aug 10 11:16:05 atala kernel: [3103663.420878] hub 3-3:1.0: 4 ports detected
^C
```

This starts streaming from the current end-of-file; ie. the client will recieve
any data appended to "syslog" after establishing the connection, but none of
the existing data.

If "syslog" is deleted or moved, tailsrv will terminate the connection. This is
the only (non-error) condition in which tailsrv will end a stream.

### Replaying "syslog" from a specific point

```
atala:~$ echo "stream syslog from byte 1000" | nc atala 4321
ctivating via systemd: service name='org.freedesktop.hostname1'
Aug 10 08:50:47 atala systemd[1]: Starting Hostname Service...
Aug 10 08:50:47 atala systemd-udevd[305]: specified group 'admin' unknown
Aug 10 08:50:47 atala dbus[1030]: [system] Successfully activated service 'org.freedesktop.hostname1'
Aug 10 08:50:47 atala systemd[1]: Started Hostname Service.
^C
```

tailsrv will send everything from byte 1000 to the end of file, and then
continue streaming new data.

If "syslog" were less than 1000 bytes long, tailsrv would have waited until it
reached 1000 bytes and then started streaming.

### Streaming "syslog", including a certain amount of replayed data

```
atala:~$ echo "stream syslog from byte -200" | nc atala 4321
sg1 type 0
Aug 10 11:16:07 atala kernel: [3103665.233587] sd 34:0:0:1: Attached scsi generic sg2 type 0
Aug 10 11:16:07 atala kernel: [3103665.246092] sd 34:0:0:0: [sdb] Attached SCSI removable disk
^C
```

tailsrv will send the last 200 bytes from the file, and then continue streaming
new data.

If "syslog" were less than 200 bytes long, tailsrc would send the whole file
and then continue streaming new data.

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
number of watched files: see `cat /proc/sys/fs/inotify/max_user_watches` (the
default is 64k). If two clients watch the same file, only one watch is used.
When all clients for a file disconnect, the watch is removed.

The server operator must ensure that all watched files are append-only. tailsrv
won't crash if you modify the middle of a file, but any expectations about log
replayability your clients may have will be broken.
