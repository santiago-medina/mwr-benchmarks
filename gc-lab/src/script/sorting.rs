//! One stable fallible merge sorter serves ordinary arrays and typed arrays.
use super::vm::{Engine, ScriptError, Value};
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn stable_sort_values(
        &mut self,
        values: &mut Vec<Value>,
        mut compare: impl FnMut(&mut Self, &Value, &Value) -> Result<i8>,
    ) -> Result<()> {
        let count = values.len();
        let mut work = vec![Value::Undefined; count];
        let mut width = 1;
        while width < count {
            for start in (0..count).step_by(width * 2) {
                let middle = (start + width).min(count);
                let end = (middle + width).min(count);
                let (mut left, mut right) = (start, middle);
                for out in start..end {
                    self.tick()?;
                    let take_left = if left == middle {
                        false
                    } else if right == end {
                        true
                    } else {
                        compare(self, &values[left], &values[right])? <= 0
                    };
                    if take_left {
                        work[out] = values[left].clone();
                        left += 1;
                    } else {
                        work[out] = values[right].clone();
                        right += 1;
                    }
                }
            }
            std::mem::swap(values, &mut work);
            width = width.saturating_mul(2);
        }
        Ok(())
    }
}
