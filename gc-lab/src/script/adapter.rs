//! Typed DOM operations. Program source and closure bodies never cross this API.
pub trait DomAdapter {
    fn create_element(&mut self, tag: &str) -> Result<u64, String>;
    fn query_selector(&mut self, selector: &str) -> Result<u64, String>;
    fn tag_name(&mut self, node: u64) -> Result<String, String>;
    fn text_content(&mut self, node: u64) -> Result<String, String>;
    fn set_text_content(&mut self, node: u64, text: &str) -> Result<(), String>;
    fn set_click_listener(&mut self, node: u64, enabled: bool) -> Result<(), String>;
    fn click(&mut self, node: u64) -> Result<Vec<u64>, String>;
    fn release(&mut self, nodes: &[u64]) -> Result<(), String>;
}
