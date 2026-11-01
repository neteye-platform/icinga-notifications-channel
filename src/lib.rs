pub mod server;

// An implementation of the [JSON-RPC 2.0](https://www.jsonrpc.org/specification) protocol for rust. Built
// for ease of use. This library is implemented in terms of future_utils streams and with that completely
// runtime agnostic.

// JSON-RPC is a stateless, light-weight remote procedure call (RPC) protocol. Primarily the specification
// defines several data structures and the rules around their processing. It is transport agnostic in that
// the concepts can be used within the same process, over sockets, over http, or in many various message
// passing environments. It uses JSON ([RFC 4627](https://www.ietf.org/rfc/rfc2119.txt)) as data format. It
// is designed to be simple!
