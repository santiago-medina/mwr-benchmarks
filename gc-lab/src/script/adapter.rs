//! Typed DOM operations. Program source and closure bodies never cross this API.
pub trait DomAdapter {
    /// Opt in only if the embedder traces VM roots into its native collector and
    /// native listeners back into the VM, then completes a coordinated sweep.
    fn traces_native_edges(&self) -> bool {
        false
    }

    /// Notification only: enqueue in the host's microtask queue, never execute inline.
    fn queue_microtask(&mut self, _serial: u64) -> Result<(), String> {
        Ok(())
    }
    /// Fetch must use the embedder's native origin/CORS/CSP-aware networking.
    fn supports_fetch(&self) -> bool {
        false
    }
    /// Start headers delivery; completion must arrive in a later host invocation.
    fn start_fetch(&mut self, _serial: u64, _url: &str) -> Result<(), String> {
        Err("native fetch unavailable".into())
    }
    fn read_response_text(&mut self, _serial: u64, _response: u64) -> Result<(), String> {
        Err("native response body unavailable".into())
    }
    fn cancel_fetch(&mut self, _serial: u64) {}
    fn release_responses(&mut self, _responses: &[u64]) -> Result<(), String> {
        Ok(())
    }
    fn create_element(&mut self, tag: &str) -> Result<u64, String>;
    fn query_selector(&mut self, selector: &str) -> Result<u64, String>;
    fn tag_name(&mut self, node: u64) -> Result<String, String>;
    fn text_content(&mut self, node: u64) -> Result<String, String>;
    fn set_text_content(&mut self, node: u64, text: &str) -> Result<(), String>;
    fn append_child(&mut self, _parent: u64, _child: u64) -> Result<(), String> {
        Err("appendChild is not supported by this adapter".into())
    }
    fn remove_child(&mut self, _parent: u64, _child: u64) -> Result<(), String> {
        Err("removeChild is not supported by this adapter".into())
    }
    fn set_click_listener(&mut self, node: u64, enabled: bool) -> Result<(), String>;
    fn click(&mut self, node: u64) -> Result<Vec<u64>, String>;
    fn release(&mut self, nodes: &[u64]) -> Result<(), String>;
}
