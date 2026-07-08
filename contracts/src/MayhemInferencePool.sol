// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {MerkleProof} from "@openzeppelin/contracts/utils/cryptography/MerkleProof.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Ownable2Step} from "@openzeppelin/contracts/access/Ownable2Step.sol";

/// @title MayhemInferencePool - Mayhem's TAP rail fund-holding contract (SAUCE direct port).
/// @notice A metered **prepaid escrow + cumulative Merkle distributor** holding ERC-20 TAP. Buyers
///         prepay; the operator (root authority) periodically posts ONE cumulative root keyed by
///         address that covers BOTH providers' 75% earnings AND buyers' refunds; everyone claims
///         their delta in O(1) (no iteration - P1). The operator's 15% fee AND the 10% burn are
///         SEPARATE, structurally capped legs (P5) - neither can exceed its cap nor touch the
///         75%/refund escrow. Value split: 75% providers / 15% operator / 10% burned.
///
///         Non-custodial (P): every rooted entitlement is claimable forever by key - `claim` is
///         permissionless and always pays the named `account`, so funds survive operator downtime;
///         only the latest UN-rooted epoch depends on a further `setRoot`. Nothing is staked.
///
///         Auditable (P6): the posted root + `cumulativeSpent` reconcile against the Trac EarnRoot
///         (same {key->cumulative} semantics); the conservation invariant below is the on-chain
///         backstop that bounds total outflow by total deposits regardless of root contents.
/// @dev C1: a single key is the deployer + owner + roller (no Safe, by design). Two mitigations cap the
///      blast radius of a stolen key on-chain: (1) `Ownable2Step` (a fat-fingered/forced ownership move
///      requires the new owner to accept), and (2) `maxEpochDelta` - one `setRoot` can add at most this much
///      new `cumulativeSpent`, so a compromised key is RATE-LIMITED per epoch rather than draining the pool
///      in one tx. Key custody + the external contract audit remain the real safeguards.
contract MayhemInferencePool is ReentrancyGuard, Ownable2Step {
    using SafeERC20 for IERC20;

    /// @notice The ERC-20 TAP this pool escrows.
    IERC20 public immutable token;

    uint256 internal constant OPERATOR_BPS = 1500; // operator share = 15%
    uint256 internal constant BURN_BPS = 1000;     // burn share = 10% (deflationary sink)
    uint256 internal constant BPS = 10_000;        // providers get the remainder = 75% (BPS-OPERATOR-BURN)
    /// @notice The 10% burn sink. `IERC20` exposes no `burn()` and standard ERC-20s revert on transfer to
    ///         `address(0)`, so the canonical dead address is used - TAP sent here is provably, permanently
    ///         out of circulation (anyone can verify its balance and the Transfer events on-chain).
    address internal constant BURN_SINK = 0x000000000000000000000000000000000000dEaD;

    /// @notice Max increase in `cumulativeSpent` allowed in a single `setRoot` (per-epoch blast-radius cap,
    ///         C1). 0 = disabled (local/dev default); set to a sane per-epoch ceiling on public networks.
    uint256 public maxEpochDelta;

    /// @notice Current cumulative distribution root (providers' earnings + buyer refunds), keyed by address.
    bytes32 public merkleRoot;
    /// @notice Monotonic epoch counter for the posted root (audit/ordering).
    uint256 public epoch;
    /// @notice Operator-declared cumulative value "spent" (value delivered). Drives the 15% cap; monotonic.
    uint256 public cumulativeSpent;

    /// @notice Cumulative amount already claimed per account (providers AND buyer refunds).
    mapping(address => uint256) public claimed;
    /// @notice sum all deposits ever made.
    uint256 public totalDeposited;
    /// @notice sum all provider/buyer claims paid out.
    uint256 public totalClaimed;
    /// @notice sum all operator (15%) withdrawals.
    uint256 public operatorWithdrawn;
    /// @notice sum all TAP burned (sent to BURN_SINK). Counts against the conservation invariant.
    uint256 public totalBurned;

    event Deposit(address indexed buyer, uint256 amount);
    event RootPosted(uint256 indexed epoch, bytes32 merkleRoot, uint256 cumulativeSpent);
    event Claimed(address indexed account, uint256 cumulativeAmount, uint256 delta);
    event OperatorWithdraw(address indexed to, uint256 amount);
    event Burned(address indexed caller, uint256 amount, uint256 totalBurned);
    event MaxEpochDeltaSet(uint256 maxEpochDelta);
    event Rescued(address indexed to, uint256 amount);

    constructor(IERC20 _token, address _owner, uint256 _maxEpochDelta) Ownable(_owner) {
        require(address(_token) != address(0), "token=0");
        token = _token;
        maxEpochDelta = _maxEpochDelta;
        emit MaxEpochDeltaSet(_maxEpochDelta);
    }

    /// @notice Owner-settable per-epoch spend cap (C1). Set this on public networks to bound a stolen key's reach.
    function setMaxEpochDelta(uint256 newMax) external onlyOwner {
        maxEpochDelta = newMax;
        emit MaxEpochDeltaSet(newMax);
    }

    // -- buyer prepay ----------------------------------------------------------------------------
    /// @notice Prepay `amount` TAP into the pool. Caller must have approved this contract.
    ///         Binds the credit to msg.sender (P4); the off-chain deposit watcher credits the buyer's
    ///         engine prepaid balance once this tx is `finalized` (Phase 5).
    function deposit(uint256 amount) external nonReentrant {
        require(amount > 0, "amount=0");
        // M11: credit the MEASURED delta, not the requested `amount`. Snapshot balanceOf before/after the
        // pull so a fee-on-transfer / non-standard ERC-20 can never credit more than the pool actually
        // received (which would break conservation). The real TAP is a plain 18-dec ERC-20, so received
        // == amount; this is strictly-safer defense that costs nothing in the normal case.
        uint256 balBefore = token.balanceOf(address(this));
        token.safeTransferFrom(msg.sender, address(this), amount);
        uint256 received = token.balanceOf(address(this)) - balBefore;
        require(received > 0, "received=0");
        totalDeposited += received;
        emit Deposit(msg.sender, received);
    }

    // -- operator posts the cumulative distribution root -------------------------------------------
    /// @notice Post the cumulative {key->owed} root. Only the owner (root authority). Monotonic epoch
    ///         and `cumulativeSpent`; spent can never exceed total deposits (sanity bound).
    function setRoot(bytes32 newRoot, uint256 newEpoch, uint256 newCumulativeSpent) external onlyOwner {
        require(newEpoch > epoch, "epoch !monotonic");
        require(newCumulativeSpent >= cumulativeSpent, "spent !monotonic");
        require(newCumulativeSpent <= totalDeposited, "spent > deposited");
        // C1: per-epoch blast-radius cap. One root can add at most `maxEpochDelta` of new spend, so a
        // compromised key is rate-limited per epoch instead of draining the pool in a single setRoot.
        if (maxEpochDelta > 0) {
            require(newCumulativeSpent - cumulativeSpent <= maxEpochDelta, "epoch delta > cap");
        }
        merkleRoot = newRoot;
        epoch = newEpoch;
        cumulativeSpent = newCumulativeSpent;
        emit RootPosted(newEpoch, newRoot, newCumulativeSpent);
    }

    // -- O(1) claim - providers' 75% earnings AND buyer refunds (same root) ------------------------
    /// @notice Claim the delta of `cumulativeAmount` over what `account` has already claimed.
    ///         Permissionless (anyone may submit; funds always go to `account`). O(1) - no iteration.
    /// @dev Leaf uses the OZ StandardMerkleTree double-hash encoding:
    ///      keccak256(bytes.concat(keccak256(abi.encode(account, cumulativeAmount)))).
    function claim(address account, uint256 cumulativeAmount, bytes32[] calldata proof) external nonReentrant {
        // Guard: never pay the zero address or the pool itself (a malformed root leaf can't strand/burn funds).
        require(account != address(0) && account != address(this), "bad account");
        bytes32 leaf = keccak256(bytes.concat(keccak256(abi.encode(account, cumulativeAmount))));
        require(MerkleProof.verify(proof, merkleRoot, leaf), "bad proof");

        uint256 already = claimed[account];
        require(cumulativeAmount > already, "nothing to claim");
        uint256 delta = cumulativeAmount - already;

        // effects
        claimed[account] = cumulativeAmount;
        totalClaimed += delta;
        // conservation backstop: total outflow (provider/refund claims + operator fee + burn) can never
        // exceed total deposits (P: no over-drain even if a malformed root over-allocates).
        require(totalClaimed + operatorWithdrawn + totalBurned <= totalDeposited, "conservation");

        // interaction
        token.safeTransfer(account, delta);
        emit Claimed(account, cumulativeAmount, delta);
    }

    // -- operator 15%, structurally capped ---------------------------------------------------------
    /// @notice Operator's still-withdrawable 15% (cap = 15% of cumulativeSpent, minus already taken).
    function operatorClaimable() public view returns (uint256) {
        uint256 cap = (cumulativeSpent * OPERATOR_BPS) / BPS; // floors -> never over-pays the operator
        return cap > operatorWithdrawn ? cap - operatorWithdrawn : 0;
    }

    /// @notice Withdraw up to the operator's capped 15%. Cannot touch the 75%/refund allocation.
    function withdrawOperator(address to, uint256 amount) external onlyOwner nonReentrant {
        require(to != address(0), "to=0");
        require(amount > 0 && amount <= operatorClaimable(), "exceeds 15% cap");

        operatorWithdrawn += amount;
        require(totalClaimed + operatorWithdrawn + totalBurned <= totalDeposited, "conservation");

        token.safeTransfer(to, amount);
        emit OperatorWithdraw(to, amount);
    }

    // -- 10% burn - deflationary sink, PERMISSIONLESS ------------------------------------------------
    /// @notice Still-burnable 10% (cap = 10% of cumulativeSpent, minus already burned). Floors -> never over-burns.
    function burnClaimable() public view returns (uint256) {
        uint256 cap = (cumulativeSpent * BURN_BPS) / BPS;
        return cap > totalBurned ? cap - totalBurned : 0;
    }

    /// @notice Burn the accrued 10% by sending it to the dead address. PERMISSIONLESS by design - anyone may
    ///         fire it (trustless deflation; the operator's roller calls it each epoch, but the burn never
    ///         depends on the operator). Structurally capped at 10% of cumulativeSpent and bounded by the
    ///         conservation invariant, so it can never touch the 75% provider/refund escrow or the 15% fee.
    function burn() external nonReentrant returns (uint256 amount) {
        amount = burnClaimable();
        require(amount > 0, "nothing to burn");

        totalBurned += amount;
        require(totalClaimed + operatorWithdrawn + totalBurned <= totalDeposited, "conservation");

        token.safeTransfer(BURN_SINK, amount);
        emit Burned(msg.sender, amount, totalBurned);
    }

    // -- rescue of NON-accounted surplus only (LOW) -------------------------------------------------
    /// @notice Tokens in the contract beyond what backs claims/refunds/operator-fee - e.g. tokens sent
    ///         directly to the pool, or fee-on-transfer dust. NEVER the escrow that backs entitlements.
    function rescuableSurplus() public view returns (uint256) {
        uint256 bal = token.balanceOf(address(this));
        uint256 accounted = totalDeposited - totalClaimed - operatorWithdrawn - totalBurned; // >=0 by the conservation invariant
        return bal > accounted ? bal - accounted : 0;
    }

    /// @notice Recover stranded surplus (<= `rescuableSurplus()`). Cannot touch escrowed entitlements.
    function rescue(address to, uint256 amount) external onlyOwner nonReentrant {
        require(to != address(0), "to=0");
        require(amount > 0 && amount <= rescuableSurplus(), "exceeds surplus");
        token.safeTransfer(to, amount);
        emit Rescued(to, amount);
    }

    // -- views (audits / non-custodial checks) -----------------------------------------------------
    /// @notice TAP currently escrowed. Invariant: poolBalance == totalDeposited - totalClaimed - operatorWithdrawn - totalBurned.
    function poolBalance() external view returns (uint256) {
        return token.balanceOf(address(this));
    }
}
