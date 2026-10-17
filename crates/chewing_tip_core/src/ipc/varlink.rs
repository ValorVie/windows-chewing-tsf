use scoped_error::{expect_error, impl_context_error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
pub struct MethodCall {
    pub method: String,
    pub parameters: Value,
    pub oneway: Option<bool>,
    pub more: Option<bool>,
    pub upgrade: Option<bool>,
}

impl std::fmt::Debug for MethodCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MethodCall")
            .field("method", &self.method)
            .field("parameters", &"<redacted>")
            .field("oneway", &self.oneway)
            .field("more", &self.more)
            .field("upgrade", &self.upgrade)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MethodReply {
    pub parameters: Value,
    pub continues: Option<bool>,
    pub error: Option<String>,
}

impl MethodReply {
    pub fn to_bytes(&self) -> Result<Vec<u8>, VarLinkError> {
        expect_error("Failed to serialize varlink MethodReply", || {
            let mut buf = serde_json::to_vec(self)?;
            buf.push(0);
            Ok(buf)
        })
    }
}

impl_context_error!(pub VarLinkError);

#[cfg(test)]
mod tests {
    use crate::ipc::d512::D512Score;

    #[test]
    fn method_call_debug_redacts_d512_text_after_conversion() {
        let message = D512Score {
            context: "合成前文不可記錄".into(),
            reading: vec!["ㄐㄧㄚˇ".into(), "ㄧˇ".into()],
            candidates: vec!["甲乙".into(), "乙甲".into()],
            remaining_micros: 100_000,
        };
        let parameters = serde_json::to_value(&message).unwrap();
        let call = message.into_call().unwrap();
        let debug = format!("{call:?}");
        for input in ["合成前文不可記錄", "ㄐㄧㄚˇ", "ㄧˇ", "甲乙", "乙甲"] {
            assert!(
                !debug.contains(input),
                "MethodCall Debug exposed input text"
            );
        }
        assert!(debug.contains("im.chewing.d512.Score"));
        assert!(debug.contains("parameters: \"<redacted>\""));
        for field in [
            "oneway: Some(false)",
            "more: Some(false)",
            "upgrade: Some(false)",
        ] {
            assert!(debug.contains(field));
        }
        // Only Debug is redacted; the scorer must still receive the full payload.
        assert_eq!(
            serde_json::to_value(&call).unwrap()["parameters"],
            parameters
        );
    }
}
