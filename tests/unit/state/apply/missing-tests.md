# Missing guard coverage in `src/core/state/State.js`

Unit tests under `tests/unit/state/apply` still lack scenarios for these guard logs:

- `handleApplyInitializeBalanceOperation`: “Invalid requester message.”
- `handleApplyDisableBalanceInitializationOperation`: “Invalid requester message.”
- `handleApplyAddAdminOperation`: “Invalid requester message.”, “Something went wrong while updating license index.”, “Something went wrong while updating writers index.”
- `handleApplyAddIndexerOperation`: “Invalid requester message.”
- `handleApplyRemoveIndexerOperation`: “Invalid requester message.”
- `handleApplyAddWriterOperation` / `#addWriter`: “Invalid requester message.”, “Invalid validator message.”, “Failed to add writer.”, “Add writer operation ignored.”, “Failed to update node entry with a writer role.”, “Something went wrong while updating writers index.”
- `handleApplyRemoveWriterOperation` / `#removeWriter`: “Invalid requester message.”, “Invalid validator message.”, “Failed to remove writer.”, “Remove writer operation ignored.”, “Failed to decode validator node entry.”
- `handleApplyBanValidatorOperation`: “Invalid requester message.”
- `handleApplyBootstrapDeploymentOperation`: “Invalid requester message.”, “Invalid validator message.”
- `handleApplyTxOperation`: “Invalid requester message.”, “Invalid validator message.”
- `handleApplyTransferOperation`: “Invalid requester message.”, “Invalid validator message.”
- `#withdrawStakedBalanceApply`: “Invalid staked balance.”, “No staked balance to unstake.”, “Invalid current balance.”, “Failed to add staked balance to current balance.”
- `#validatorPenaltyApply`: “Admin entry not found.”, “Admin cannot be penalized.”
- `#transferFeeTxOperation`: “Invalid incoming data.”
