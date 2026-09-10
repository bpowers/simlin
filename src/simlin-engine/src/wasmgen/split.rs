// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//! Partition encoded opcode programs at the lowering pass's safe boundaries.

use wasm_encoder::{Function, Instruction};

use super::WasmGenError;

/// A sizing policy for ordinary helper bodies, independent of the engine's
/// acceptance limit. An indivisible expression may exceed this target.
pub(super) const TARGET_BODY_BYTES: usize = 64 * 1024;

/// V8's `kV8MaxWasmFunctionSize`, including the local-declaration prefix:
/// https://github.com/v8/v8/blob/main/src/wasm/wasm-limits.h
/// Enforce this on every emitted body so an oversized indivisible expression
/// or driver returns a compiler error rather than failing in the JS host.
const MAX_BODY_BYTES: usize = 7_654_321;

/// A partition shares the original opcode program's parameter signature.
pub(super) struct ProgramHelper {
    pub n_inputs: u32,
    pub body: Function,
}

/// Own the function indices and bodies appended after the fixed phase/driver
/// functions. Existing calls keep targeting the original phase entry points.
pub(super) struct ProgramSplitter {
    first_index: u32,
    target_bytes: usize,
    max_bytes: usize,
    pub functions: Vec<ProgramHelper>,
}

impl ProgramSplitter {
    pub fn new(first_index: u32, target_bytes: usize) -> Self {
        Self {
            first_index,
            target_bytes: target_bytes.min(MAX_BODY_BYTES),
            max_bytes: MAX_BODY_BYTES,
            functions: Vec::new(),
        }
    }

    /// Keep small functions intact. Large functions become a sequence of calls
    /// whose bodies retain the original local numbering and instruction bytes.
    /// `frame` contains only local declarations; `body` includes its final End;
    /// `safe_offsets` are absolute encoded-body offsets supplied by lowering.
    pub fn finish(
        &mut self,
        frame: Function,
        body: Function,
        safe_offsets: &[usize],
        n_inputs: u32,
    ) -> Result<Function, WasmGenError> {
        if body.byte_len() <= self.target_bytes {
            return Ok(body);
        }
        let prefix_len = frame.byte_len();
        let code_end = body.byte_len() - 1;
        if code_end == prefix_len {
            check_size(&body, self.max_bytes)?;
            return Ok(body);
        }
        if safe_offsets.last().copied() != Some(code_end) {
            return Err(WasmGenError::Unsupported(
                "wasmgen: opcode program ends with live evaluation state".to_string(),
            ));
        }
        if !safe_offsets
            .iter()
            .any(|&end| end > prefix_len && end < code_end)
        {
            if body.byte_len() > self.max_bytes {
                return Err(WasmGenError::Unsupported(format!(
                    "wasmgen: indivisible expression exceeds the {}-byte function limit",
                    self.max_bytes
                )));
            }
            return Ok(body);
        }
        let raw = body.into_raw_body();

        let mut dispatcher = Function::new([]);
        let mut start = prefix_len;
        let mut previous = start;
        for &end in safe_offsets {
            if end <= start {
                continue;
            }
            if end - start + prefix_len + 1 > self.target_bytes && previous > start {
                self.append(&frame, &raw[start..previous], n_inputs, &mut dispatcher)?;
                start = previous;
            }
            // A single expression cannot be cut while its operand/local state
            // is live. It may exceed the target, but never the host's limit.
            if end - start + prefix_len + 1 > self.max_bytes {
                return Err(WasmGenError::Unsupported(format!(
                    "wasmgen: indivisible expression exceeds the {}-byte function limit",
                    self.max_bytes
                )));
            }
            previous = end;
        }
        if start < code_end {
            self.append(&frame, &raw[start..code_end], n_inputs, &mut dispatcher)?;
        }
        dispatcher.instruction(&Instruction::End);
        check_size(&dispatcher, self.max_bytes)?;
        Ok(dispatcher)
    }

    fn append(
        &mut self,
        frame: &Function,
        code: &[u8],
        n_inputs: u32,
        dispatcher: &mut Function,
    ) -> Result<(), WasmGenError> {
        let index = u32::try_from(self.functions.len())
            .ok()
            .and_then(|n| self.first_index.checked_add(n))
            .ok_or_else(|| {
                WasmGenError::Unsupported("wasmgen: too many function partitions".to_string())
            })?;
        let mut body = frame.clone();
        body.raw(code.iter().copied());
        body.instruction(&Instruction::End);
        check_size(&body, self.max_bytes)?;
        self.functions.push(ProgramHelper { n_inputs, body });
        for param in 0..=n_inputs {
            dispatcher.instruction(&Instruction::LocalGet(param));
        }
        dispatcher.instruction(&Instruction::Call(index));
        Ok(())
    }
}

/// This gate also covers helpers and drivers not emitted from opcode programs.
pub(super) fn check_function(body: &Function) -> Result<(), WasmGenError> {
    check_size(body, MAX_BODY_BYTES)
}

fn check_size(body: &Function, max_bytes: usize) -> Result<(), WasmGenError> {
    if body.byte_len() > max_bytes {
        return Err(WasmGenError::Unsupported(format!(
            "wasmgen: generated function is {} bytes, exceeding the {max_bytes}-byte limit",
            body.byte_len()
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "split_unit_tests.rs"]
mod tests;
