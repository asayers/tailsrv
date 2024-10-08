## 0.8.0

* There's a new "tsmirror" example program
* tscat (and tsmirror) now use TCP_KEEPALIVE to detect a dead connection
* Tailsrv no longer reads data sent to it by clients

The point of this feature was to allow client to detect a dead connection by
periodically sending dummy data to the server.  However, it turns out that's
exactly what TCP_KEEPALIVE is for!  Removing the reading functionality is a
simplification (as evidenced by the bugfix in 0.7.1).

## v0.7.2

No user-visible changes.

## v0.7.1

* Client connections will be closed upon encountering an I/O error.

  tailsrv works by `sendfile()`-ing the watched file to the clients.  It's
  possible for this function to return an error (eg. if the watched file is on
  a dying hard disk).  In this case, we don't know what data was actually sent
  to the client, and can't reasonably continue.  The unknown-state client is
  therefore terminated.  In v0.7.0, however, the connection would remain alive
  - the client would simply not receive any new data after the error occurred.
  In v0.7.1, the connection will be closed promptly, so that the client can
  establish a new connection (and thereby let tailsrv know its current offset).

## v0.7.0

* Tailsrv now reads (and discards) any data send to it by clients.  This allows
  clients to their TCP connections by attempting to send some dummy data
  to tailsrv.  If the connection is silently dead, the write will fail.
* Logs are printed to stderr instead of stdout.
* The repo now includes an example client program called "tscat".
