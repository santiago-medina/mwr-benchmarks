//! Line protocol for the experimental Servo adapter. Strings are UTF-8 hex data;
//! there is no operation for evaluating program source or closure bodies.
use super::DomAdapter;
use std::io::{self, Write};

pub fn encode(text: &str) -> String {
    text.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
pub fn decode(text: &str) -> Result<String, String> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid hex payload".into());
    }
    let bytes: Result<Vec<_>, _> = text
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16))
        .collect();
    String::from_utf8(bytes.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
pub struct LineAdapter;
impl LineAdapter {
    fn request(&mut self, operation: &str, args: &[String]) -> Result<String, String> {
        let mut line = format!("CALL\t{operation}");
        for arg in args {
            line.push('\t');
            line.push_str(arg);
        }
        println!("{line}");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut response = String::new();
        if io::stdin()
            .read_line(&mut response)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("adapter disconnected".into());
        }
        let response = response.trim_end_matches(['\n', '\r']);
        let (status, value) = response.split_once('\t').unwrap_or((response, ""));
        match status {
            "OK" => Ok(value.into()),
            "ERR" => Err(decode(value)?),
            _ => Err("invalid adapter response".into()),
        }
    }
    fn number(&mut self, operation: &str, args: &[String]) -> Result<u64, String> {
        self.request(operation, args)?
            .parse()
            .map_err(|_| "invalid remote node ID".into())
    }
}
impl DomAdapter for LineAdapter {
    fn create_element(&mut self, tag: &str) -> Result<u64, String> {
        self.number("CREATE", &[encode(tag)])
    }
    fn query_selector(&mut self, selector: &str) -> Result<u64, String> {
        self.number("QUERY", &[encode(selector)])
    }
    fn tag_name(&mut self, node: u64) -> Result<String, String> {
        decode(&self.request("TAG", &[node.to_string()])?)
    }
    fn text_content(&mut self, node: u64) -> Result<String, String> {
        decode(&self.request("TEXT", &[node.to_string()])?)
    }
    fn set_text_content(&mut self, node: u64, text: &str) -> Result<(), String> {
        self.request("SET_TEXT", &[node.to_string(), encode(text)])
            .map(|_| ())
    }
    fn set_click_listener(&mut self, node: u64, enabled: bool) -> Result<(), String> {
        self.request(
            "LISTEN",
            &[node.to_string(), if enabled { "1" } else { "0" }.into()],
        )
        .map(|_| ())
    }
    fn click(&mut self, node: u64) -> Result<Vec<u64>, String> {
        let response = self.request("CLICK", &[node.to_string()])?;
        if response.is_empty() {
            return Ok(vec![]);
        }
        response
            .split(',')
            .map(|s| s.parse().map_err(|_| "invalid event node ID".into()))
            .collect()
    }
    fn release(&mut self, nodes: &[u64]) -> Result<(), String> {
        self.request(
            "RELEASE",
            &[nodes
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")],
        )
        .map(|_| ())
    }
}
