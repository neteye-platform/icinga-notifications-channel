use erased_serde::Serialize as DynSerialize;
use monostate::MustBe;
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

#[derive(Deserialize)]
pub struct JsonRpcRequest<'request> {
    jsonrpc: MustBe!("2.0"),
    method: String,
    #[serde(borrow)]
    params: &'request RawValue,
    id: Option<Value>,
}

#[derive(Serialize)]
pub enum JsonRpcResult {
    #[serde(rename = "result")]
    Ok(Box<dyn DynSerialize>),
    #[serde(rename = "error")]
    Err {
        code: JsonRpcErrorCode,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Box<dyn DynSerialize>>,
    },
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    jsonrpc: MustBe!("2.0"),
    #[serde(flatten)]
    result: JsonRpcResult,
    id: Value,
}

#[derive(Serialize)]
pub struct JsonRpcErrorCode(i32);

impl JsonRpcErrorCode {
    pub const fn error_code(&self) -> i32 {
        self.0
    }

    pub const fn parse_error() -> Self {
        Self(-32700)
    }

    pub const fn invalid_request() -> Self {
        Self(-32600)
    }

    pub const fn method_not_found() -> Self {
        Self(-32601)
    }

    pub const fn invalid_params() -> Self {
        Self(-32602)
    }

    pub const fn internal_error() -> Self {
        Self(-32603)
    }

    pub const fn server_error(error_code: i32) -> Self {
        Self(error_code)
    }

    pub const fn custom_error(error_code: i32) -> Self {
        Self(error_code)
    }
}

#[cfg(test)]
mod tests {
    use monostate::MustBe;
    use serde_json::json;

    use crate::server::{JsonRpcErrorCode, JsonRpcRequest, JsonRpcResponse, JsonRpcResult};

    #[test]
    pub fn should_deserialize_basic_request() {
        let request_str =
            r#"{"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_str).unwrap();
        assert_eq!(request.method.as_str(), "subtract");
        assert_eq!(request.params.get(), "[42, 23]")
    }

    #[test]
    pub fn should_serialize_result() {
        let response = JsonRpcResponse {
            jsonrpc: MustBe!("2.0"),
            result: JsonRpcResult::Ok(Box::new(19)),
            id: json!(1),
        };

        let response_str = serde_json::to_string(&response).unwrap();
        assert_eq!(
            response_str.as_str(),
            r#"{"jsonrpc":"2.0","result":19,"id":1}"#
        );
    }

    #[test]
    pub fn should_serialize_error() {
        let error_response = JsonRpcResponse {
            jsonrpc: MustBe!("2.0"),
            result: JsonRpcResult::Err {
                code: JsonRpcErrorCode::method_not_found(),
                message: "Method not found".to_string(),
                data: None,
            },
            id: json!("1"),
        };
        let response_str = serde_json::to_string(&error_response).unwrap();
        assert_eq!(
            response_str.as_str(),
            r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":"1"}"#
        );
    }
}
