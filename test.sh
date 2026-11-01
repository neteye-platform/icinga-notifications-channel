#!/usr/bin/env bash

# This test checks all the examples from the jsonrpc example section https://www.jsonrpc.org/specification#examples
# against the server.
cargo build --example nl_jsonrpc


function test_request() {
    REQUEST="$1"
    EXPECTED_RESPONSE=$(echo $2 | jq -c)

    echo "--> $REQUEST"
    RESPONSE="$(echo "$REQUEST" | target/debug/examples/nl_jsonrpc | jq -c "${3:-.}")"
    test -n "$RESPONSE" && echo "<-- $RESPONSE"

    if [ "$RESPONSE" != "$(echo "$EXPECTED_RESPONSE" | jq -c)" ] ; then
        echo
        echo '[!] Test did not pass!'
        echo "EXPECTED: '$EXPECTED_RESPONSE'"
        echo "GOT:      '$RESPONSE'"
        exit 1
    fi
}

echo 'rpc call with positional parameters:'
test_request \
    '{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}' \
    '{"jsonrpc": "2.0", "result": 19, "id": 1}'

test_request \
    '{"jsonrpc": "2.0", "method": "subtract", "params": [23, 42], "id": 2}' \
    '{"jsonrpc": "2.0", "result": -19, "id": 2}'

echo
echo 'rpc call with named parameters:'
test_request \
    '{"jsonrpc": "2.0", "method": "subtract", "params": {"subtrahend": 23, "minuend": 42}, "id": 3}' \
    '{"jsonrpc": "2.0", "result": 19, "id": 3}'

test_request \
    '{"jsonrpc": "2.0", "method": "subtract", "params": {"minuend": 42, "subtrahend": 23}, "id": 4}' \
    '{"jsonrpc": "2.0", "result": 19, "id": 4}'

echo
echo 'a Notification:'
test_request \
    '{"jsonrpc": "2.0", "method": "update", "params": [1,2,3,4,5]}' \
    ''

test_request \
    '{"jsonrpc": "2.0", "method": "foobar"}' \
    ''

echo
echo 'rpc call of non-existent method:'
test_request \
    '{"jsonrpc": "2.0", "method": "foobar", "id": "1"}' \
    '{"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "1"}' \
    'del(.error.data)'

echo
echo "rpc call with invalid JSON:"
test_request \
    '{"jsonrpc": "2.0", "method": "foobar, "params": "bar", "baz]' \
    '{"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}, "id": null}' \
    'del(.error.data)'

echo
echo "rpc call with invalid Request object:"
test_request \
    '{"jsonrpc": "2.0", "method": 1, "params": "bar"}' \
    '{"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}' \
    'del(.error.data)'

echo
echo 'rpc call Batch, invalid JSON:'
test_request \
    '[{"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},{"jsonrpc": "2.0", "method"]' \
    '{"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}, "id": null}' \
    'del(.error.data)'

echo
echo 'rpc call with an empty Array:'
test_request \
    '[]' \
    '{"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}'

echo
echo 'rpc call with an invalid Batch (but not empty):'
test_request \
    '[1]' \
    '[
      {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}
    ]' \
    'del(.[].error.data)'

test_request \
    '[1,2,3]' \
    '[
      {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null},
      {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null},
      {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}
    ]' \
    'del(.[].error.data)'

echo
echo 'rpc call Batch:'
REQUEST='[
    {"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},
    {"jsonrpc": "2.0", "method": "notify_hello", "params": [7]},
    {"jsonrpc": "2.0", "method": "subtract", "params": [42,23], "id": "2"},
    {"foo": "boo"},
    {"jsonrpc": "2.0", "method": "foo.get", "params": {"name": "myself"}, "id": "5"},
    {"jsonrpc": "2.0", "method": "get_data", "id": "9"}
]'
test_request \
    "$(echo "$REQUEST" | jq -c)" \
    '[
        {"jsonrpc": "2.0", "result": 7, "id": "1"},
        {"jsonrpc": "2.0", "result": 19, "id": "2"},
        {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null},
        {"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "5"},
        {"jsonrpc": "2.0", "result": ["hello", 5], "id": "9"}
    ]' \
    'del(.[].error.data)'

echo
echo 'rpc call Batch (all notifications):'
test_request \
    '[{"jsonrpc": "2.0", "method": "notify_sum", "params": [1,2,4]},{"jsonrpc": "2.0", "method": "notify_hello", "params": [7]}]' \
    ''
