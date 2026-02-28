use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256};
use revm::bytecode::opcode;
use revm::interpreter::interpreter::EthInterpreter;
use revm::interpreter::interpreter_types::{InputsTr, Jumps};
use revm::interpreter::Interpreter;
use revm::Inspector;

/// Records all state accesses during EVM execution.
///
/// Tracks which addresses are accessed (via BALANCE, EXTCODEHASH, etc.)
/// and which storage slots are read or written (via SLOAD, SSTORE).
/// Results are deduplicated and sorted deterministically.
#[derive(Debug, Default)]
pub struct AccessListInspector {
    /// Address -> set of storage slots accessed.
    /// An address with an empty set means account-level access only.
    accesses: BTreeMap<Address, BTreeSet<B256>>,
}

impl AccessListInspector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_accesses(self) -> BTreeMap<Address, BTreeSet<B256>> {
        self.accesses
    }
}

impl<CTX> Inspector<CTX, EthInterpreter> for AccessListInspector {
    fn step(&mut self, interp: &mut Interpreter<EthInterpreter>, _context: &mut CTX) {
        let op = interp.bytecode.opcode();

        match op {
            opcode::SLOAD | opcode::SSTORE => {
                // Stack: [slot, ...] — slot is at top
                let address = interp.input.target_address();
                if let Ok(slot_u256) = interp.stack.peek(0) {
                    let slot = B256::from(slot_u256);
                    self.accesses.entry(address).or_default().insert(slot);
                }
            }
            opcode::BALANCE | opcode::EXTCODEHASH | opcode::EXTCODESIZE | opcode::EXTCODECOPY => {
                // Stack: [address, ...] — address is at top
                if let Ok(addr_u256) = interp.stack.peek(0) {
                    let addr = Address::from_word(B256::from(addr_u256));
                    self.accesses.entry(addr).or_default();
                }
            }
            opcode::SELFBALANCE => {
                let address = interp.input.target_address();
                self.accesses.entry(address).or_default();
            }
            _ => {}
        }
    }
}
