=======
tailsrv
=======

A server which allows clients to tail files in the working directory

SYNOPSIS
--------

**tailsrv** --port <PORT>

DESCRIPTION
-----------

tailsrv is a high-performance file-streaming server.  It's like `tail -f` in
server form.  It has high throughput, low latency, and scales to lots of
clients (see [Performance](#performance)).  Setup is very simple, and clients
don't require a special library.  It is, however, Linux-only (see
[Limitations](#limitations)).

Client connections are monitored with epoll.  Files are monitored with inotify.
When a file changes or a connection becomes writable, tailsrv `sendfile()` s the
new data.

OPTIONS
-------

-p <PORT>, --port=<PORT>
    The port on which tailsrc listens for new TCP connections from clients.

-v
    Increases the level of verbosity (may be specified multiple times)

-q, --quiet
    Don't print anything unless there's a problem

-h, --help
    Prints help information

-V, --version
    Prints version information

RULE-SENDING PROTOCOL
---------------------

The DC listens for rules on UDP port 14324. Whenever the DC recieves a packet
on that port, the rules specified by that packet completely replace the
existing set of rules.

A ruleset is a concatination of rules. A rule is encoded like this::

    T<side><price><length><tx data><length><tag data>

The side is an ASCII 'b' (bid) or 'o' (offer). The price is an i32. The lengths
are u32s. Everything is big-endian.

DISCUSSION
----------

The idea is to run two parallel trading systems: one which does all the complex
logic related to pricing and strategies, and another very simple one optimised
for latency which watches for certain market events and responds to them as
quickly as possible.

On not running DC
~~~~~~~~~~~~~~~~~

The DC code gives the following penalties *even when the DC isn't running*:

1. ordergen is 1% slower;
2. nomserver has 5 fewer user IDs.

If you're planning not to run the DC, you can remove these penalties completely

