# tailsrv vs Kafka

Kafka does three things:

1. **Collating** messages from multiple producers into a single stream
2. **Indexing** streams by message number
3. **Broadcasting** streams to consumers

(Actually Kafka does many many things, but these are the main ones.)

tailsrv only handles broadcasting.  If you need collating or indexing
functionality, you should roll some software for that yourself and run it
alongside tailsrv on the fileserver.

## Collating

Kafka provides an API for reading streams, and also an API for writing to
them.  tailsrv only does the reading side: it doesn't help you coalesce data.
For this you'll need to roll your own solution.

If your data comes from a single process on a single machine, it's dead easy:
you just need to get the data over to the fileserver somehow.  If your data
comes from multiple sources and needs to be carefully aggregated into a single
stream, then you'll need to run another piece of software on fileserver
which accepts connections from your producers and writes the collated data
into a file.

## Indexing

Kafka's abstraction is "streams of messages", whereas tailsrv's abstraction is
"a stream of bytes".  If you want to chop your stream up into messages, just
do that however you'd like (newline-delimited, length-prefixed, etc. etc.).
However, tailsrv doesn't know about your messages, so can't provide indexing
for them.  This means that, if you want to start reading from a certain
message, you have to know its byte-offset.  You could do this with another
piece of software on the fileserver (an indexer).  Kafka does this for you,
but tailsrv doesn't so you'll have to roll your own.

## Encryption

Kafka can encrypt streams.  tailsrv doesn't do that.

If you want encrypted transport, [use a VPN][wireguard].
Don't trust me with your crypto - just make sure the route to your fileserver is secure.

Want to completely prohibit insecure access?  Run tailsrv in a network namespace
which doesn't contain any non-vpn network interfaces.

[wireguard]: https://www.wireguard.com

## Authentication

Kafka offers ACLs for sophisticated access control.  tailsrv doesn't do that.

If your authentication requirements are simple, using a VPN solves this too.

## Multiple files

Kafka handles multiple streams with a single server instance.
tailsrv is strictly one-file-per-server.

If you want to stream multiple files, you can just run multiple instances.
Since tailsrv is so lightweight, this is actually a reasonable option, for less than (say) 1000 files.
The main problem is you'll have to communicate the port-number ⟷ filename mapping to the clients somehow.

## High availablility

If you need writen data to remain available when your fileserver dies, just
replicate it to another machine which is also running tailsrv.  (If you
have two machines physically next to each other, how about using DRBD?)
If the server dies, so too will clients' connections.  So long as they're
keeping track of their position in the log, they can connect to the backup
server and carry on.  If you want failover to be transparent to the clients,
you could stick a reverse proxy in front of the tailsrv instances, or do
something fancy with DNS.

## Extremely high throughput

Kafka is designed to handle throughputs which would be too much for a
single fileserver.  If you're in that kind of situation, then I'm sorry!
tailsrv won't work for you.

## Extremely large files

Suppose you're running tailsrv on a file that gets rotated.  If the file
is moved then tailsrv will exit.  If it's truncated then tailsrv will keep
going, but won't send clients any more data (until the file exceeds its
previous length).  Either way, it doesn't work well.

## Conclusion

The take-away here is that tailsrv can act as _one component_ in a Kafka-like
setup.  Need indexing?  Add an indexer.  Need encryption?  Add wireguard.
Keep adding components until the system does what you need.

The advantage is that, if you only require a small subset of Kafka's
functionality, the system you end up with will be much simpler and easier
to work with.

The caveat: if you need Kafka's horizontal scalability, then you should just use Kafka (or something like it).
It's worth mentioning, though: organisations which need a cluster to sustain their throughput are the exception, not the rule.
If you're in this situation, then you know it (and are sad about it).
If you're not sure whether your data streams require a cluster, then they probably don't.
