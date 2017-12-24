**So... we NIH'd Kafka**. TL;DR: If you want to stream log files from a central
location to many consumers, but don't need Kafka's other features, read on for
a low-complexity alternative.

We used to use Kafka for storing logs and streaming updates to consumers. Kafka
worked fine, and as far as I can tell it's a well-designed, well-written piece
of software.

The design of tailsrv is quite different to Kafka, but it can be used to
achieve similar goals. As you may have noticed, tailsrv does a lot less than
Kafka. If you need the feasures listed below, stick with Kafka.
