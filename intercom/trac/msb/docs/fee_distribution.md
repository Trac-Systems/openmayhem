# Fee distribution (state.js)

## Constants
- `FEE` = 0.03 $TNK base fee.
- Predefined splits: `PERCENT_75`, `PERCENT_50`, `PERCENT_25`.

## Operations (excluding TRANSFER and TX)
| Operation | Who pays / amount | When charged | Validator | Remainder |
| --- | --- | --- | --- | --- |
| AddAdmin | Admin, `0` | One-time during initialization | 0% | n/a (free) |
| BalanceInitialization | Admin (sender), `0` | While initialization flag is enabled | 0% | n/a (free) |
| AppendWhitelist | Admin, `FEE` | Only if initialization flag is disabled | 0% | 100% burned (deducted from admin) |
| AddWriter | Writer candidate, `FEE` | Always charged | 75% | 25% burned |
| RemoveWriter | Writer being removed, `FEE` | Always charged | 75% | 25% burned |
| AdminRecovery | Admin, `FEE` | Always charged | 75% | 25% burned |
| AddIndexer | Admin, `FEE` | Always charged | 0% | 100% burned |
| RemoveIndexer | Admin, `FEE` | Always charged | 0% | 100% burned |
| BanValidator | Admin, `FEE` | Always charged | 0% | 100% burned |
| BootstrapDeployment | Deployment initiator, `FEE` | Always charged | 75% | 25% burned |
| Transfer | Requester, `FEE` | Always charged | 75% | 25% burned |
| TX | Requester, `FEE` | Always charged | depends on subnet owner (see TX cases) | depends on subnet owner (see TX cases) |

### Operation details
- **AddAdmin**
  - Payer: admin (network creator), amount: free.
  - Effect: admin receives initial balance `1000 $TNK` and initial staked balance `0.3 $TNK`; admin is set as indexer+validator+whitelisted and admin entry is created. Runs exactly once during bootstrap.
- **AddWriter**
  - Payer: writer candidate, amount: `FEE`.
  - Split: validator 75%, burned 25%.
  - Effect: role set to writer, staking updated, key registered.
- **RemoveWriter**
  - Payer: writer being removed, amount: `FEE`.
  - Split: validator 75%, burned 25%.
  - Effect: role set to whitelisted, stake released, writer key unregistered.
- **AdminRecovery**
  - Payer: admin rotating writer key, amount: `FEE`.
  - Split: validator 75%, burned 25%.
  - Effect: admin writer key swapped, indexer entry updated to the new key.
- **BootstrapDeployment**
  - Payer: deployment initiator, amount: `FEE`.
  - Split: validator 75%, burned 25%.
  - Effect: deployment entry stored for the bootstrap.

## Transfer (OperationType.TRANSFER)
- Payer: requester (sender).
- Fee: `FEE` (always charged, in self-transfer only the fee is deducted).
- Basics:
  - Amount deducted from sender: `transferAmount + FEE`. In self-transfer only the fee is deducted.
  - If the sender lacks full funds for the deduction, the operation is ignored (state unchanged).
- Fee split: validator 75%, burned 25%.
- Transfer amount to recipient:
  - Recipient is not a validator: recipient gets `transferAmount` (can be 0). A new recipient is initialized as READER with that balance.
  - Recipient is the validator: validator gets `transferAmount` plus its fee share (75% of `FEE`).
  - Self-transfer: recipient equals sender. Recipient balance is unchanged. Fee still goes 75% to validator, 25% burned.

## Subnetwork TX (OperationType.TX)
- Payer: requester.
- Fee: `FEE`.
- Fee split depends on who deployed the subnet (`bootstrapDeployer`):
  1. `bootstrapDeployer = requester`, validator is different:
     - Validator: 50%
     - Bootstrap deployer: 0% (no discount for owning the subnet)
     - Burned: 50%
  2. `bootstrapDeployer = validator`, requester is different:
     - Validator (and deployer): 75% (50% as validator + 25% as deployer)
     - Burned: 25%
     - Requester: 0%
  3. `bootstrapDeployer` is neither requester nor validator:
     - Validator: 50%
     - Bootstrap deployer: 25%
     - Burned: 25%
     - Requester: 0%
- The fee is deducted from the requester before distribution. Others receive only the shares above.

## Zero-fee operations
- `ADD_ADMIN` (one-time bootstrap, assigns `1000 $TNK` liquid + `0.3 $TNK` staked to admin).
- `BALANCE_INITIALIZATION` (sender -> recipient top-up) is free while initialization is enabled.
- `DISABLE_INITIALIZATION` is free, can be used exactly once to turn off balance initialization.
- AppendWhitelist before initialization is disabled is free (fee appears only after the flag is turned off).

## Validator penalties
- For a batch with invalid operations or an oversized batch: `penalty = FEE * invalidOperations`.
- The penalty is taken from staked balance, not distributed, and the validator is downgraded to whitelisted. Remaining stake after the penalty returns to the normal balance, so part of the penalty is effectively burned.
- Each failed operation in a batch (validation errors, unmet conditions, invalid payload) increases the invalid counter and triggers at least one `FEE` worth of penalty.

## Note on the `bs` field in TX
- `bs` identifies the subnet bootstrap, mapped in MSB to the subnet creator address.
- A TX is valid only if `bs` points to a registered bootstrap.
- The subnet owner (address tied to `bs`) receives a share of the TX fee only when they are not the requester. If requester or validator is the creator, the creator’s share is folded into their respective percentage as described above.
