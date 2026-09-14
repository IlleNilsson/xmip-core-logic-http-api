# xmip-core-logic-http-api

The `http-api` logic technology, a technology of
[xmip-core-logic](https://github.com/IlleNilsson/xmip-core-logic): a method and a path: an OpenAPI document names each as an operationId and yields path parameters; a result is 200 with its body, a fault its status with a problem body. This is what retired xmip-core-webapi.

What a Logic technology is — the method, between the transport that moves the
bytes and the contract that types the content — is ADR-0043. Both directions
live here: a Receive Location reads invocations and writes replies, a Send
Location writes requests and reads outcomes.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
