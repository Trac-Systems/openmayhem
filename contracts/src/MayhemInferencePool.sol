// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {MerkleProof} from "@openzeppelin/contracts/utils/cryptography/MerkleProof.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
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
///         only the latest UN-rooted epoch depends on another governed root proposal. Nothing is staked.
///
///         Auditable (P6): the posted root + `cumulativeSpent` reconcile against the Trac EarnRoot
///         (same {key->cumulative} semantics); the conservation invariant below is the on-chain
///         backstop that bounds total outflow by total deposits regardless of root contents.
/// @dev Root, cap, and rescue authority is deliberately split: the owner proposes an action carrying an
///      independent EIP-712 governance signature, then anyone executes it after the immutable delay. A
///      stolen owner key therefore cannot post a payout root, disable the epoch cap, or rescue surplus by
///      itself. `maxEpochDelta` remains the per-root value blast-radius bound.
contract MayhemInferencePool is ReentrancyGuard, Ownable2Step {
    using SafeERC20 for IERC20;

    /// @notice The ERC-20 TAP this pool escrows.
    IERC20 public immutable token;

    uint256 internal constant OPERATOR_BPS = 1500; // operator share = 15%
    uint256 internal constant PROVIDER_BPS = 7500; // provider share = 75%
    uint256 internal constant BPS = 10_000;        // providers get the remainder = 75% (BPS-OPERATOR-BURN)
    uint64 internal constant MIN_PUBLIC_GOVERNANCE_DELAY = 1 hours;
    /// @notice The 10% burn sink. `IERC20` exposes no `burn()` and standard ERC-20s revert on transfer to
    ///         `address(0)`, so the canonical dead address is used - TAP sent here is provably, permanently
    ///         out of circulation (anyone can verify its balance and the Transfer events on-chain).
    address internal constant BURN_SINK = 0x000000000000000000000000000000000000dEaD;

    /// @notice Max increase in `cumulativeSpent` allowed in one root.
    ///         Zero is permitted only on recognized local development chains.
    uint256 public maxEpochDelta;

    /// @notice Independent signer required alongside the owner for governed actions.
    address public immutable governanceSigner;
    /// @notice Immutable delay between a governed proposal and permissionless execution.
    uint64 public immutable governanceDelay;
    /// @notice Global EIP-712 proposal nonce; each accepted signature is usable exactly once.
    uint256 public governanceNonce;

    bytes32 internal constant ROOT_PROPOSAL_TYPEHASH = keccak256(
        "RootProposal(bytes32 merkleRoot,uint256 newEpoch,uint256 newCumulativeSpent,uint256 previousEpoch,uint256 previousCumulativeSpent,uint256 nonce)"
    );
    bytes32 internal constant EIP712_DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 internal constant EIP712_NAME_HASH = keccak256("MayhemInferencePool");
    bytes32 internal constant EIP712_VERSION_HASH = keccak256("1");
    bytes32 internal constant MAX_EPOCH_DELTA_PROPOSAL_TYPEHASH =
        keccak256("MaxEpochDeltaProposal(uint256 newMaxEpochDelta,uint256 nonce)");
    bytes32 internal constant RESCUE_PROPOSAL_TYPEHASH =
        keccak256("RescueProposal(address to,uint256 amount,uint256 nonce)");

    struct PendingRoot {
        bytes32 merkleRoot;
        uint256 newEpoch;
        uint256 newCumulativeSpent;
        uint256 previousEpoch;
        uint256 previousCumulativeSpent;
        uint256 nonce;
        uint64 executeAfter;
    }

    struct PendingMaxEpochDelta {
        uint256 newMaxEpochDelta;
        uint256 nonce;
        uint64 executeAfter;
    }

    struct PendingRescue {
        address to;
        uint256 amount;
        uint256 nonce;
        uint64 executeAfter;
    }

    PendingRoot public pendingRoot;
    PendingMaxEpochDelta public pendingMaxEpochDelta;
    PendingRescue public pendingRescue;

    /// @notice Current cumulative distribution root (providers' earnings + buyer refunds), keyed by address.
    bytes32 public merkleRoot;
    /// @notice Append-only roots. A later root can never strand an unclaimed entitlement from an older one.
    mapping(uint256 => bytes32) public merkleRootAtEpoch;
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
    event RootProposed(
        uint256 indexed epoch,
        bytes32 merkleRoot,
        uint256 cumulativeSpent,
        uint256 indexed nonce,
        uint64 executeAfter
    );
    event RootPosted(uint256 indexed epoch, bytes32 merkleRoot, uint256 cumulativeSpent, uint256 indexed nonce);
    event Claimed(uint256 indexed rootEpoch, address indexed account, uint256 cumulativeAmount, uint256 delta);
    event OperatorWithdraw(address indexed to, uint256 amount);
    event Burned(address indexed caller, uint256 amount, uint256 totalBurned);
    event MaxEpochDeltaProposed(uint256 maxEpochDelta, uint256 indexed nonce, uint64 executeAfter);
    event MaxEpochDeltaSet(uint256 maxEpochDelta);
    event RescueProposed(address indexed to, uint256 amount, uint256 indexed nonce, uint64 executeAfter);
    event Rescued(address indexed to, uint256 amount);

    constructor(
        IERC20 _token,
        address _owner,
        address _governanceSigner,
        uint64 _governanceDelay,
        uint256 _maxEpochDelta
    ) Ownable(_owner) {
        require(address(_token) != address(0), "token=0");
        require(_governanceSigner != address(0), "governance signer=0");
        require(_governanceSigner != _owner, "governance signer=owner");
        if (!_isLocalDevelopmentChain()) {
            require(_governanceDelay >= MIN_PUBLIC_GOVERNANCE_DELAY, "governance delay too short");
            require(_maxEpochDelta > 0, "max epoch delta=0");
        }
        token = _token;
        governanceSigner = _governanceSigner;
        governanceDelay = _governanceDelay;
        maxEpochDelta = _maxEpochDelta;
        emit MaxEpochDeltaSet(_maxEpochDelta);
    }

    function _isLocalDevelopmentChain() internal view returns (bool) {
        return block.chainid == 1337 || block.chainid == 31337 || block.chainid == 61000;
    }

    function _executeAfter() internal view returns (uint64) {
        uint256 value = block.timestamp + governanceDelay;
        require(value <= type(uint64).max, "execute time overflow");
        return uint64(value);
    }

    function _domainSeparator() internal view returns (bytes32) {
        return keccak256(
            abi.encode(EIP712_DOMAIN_TYPEHASH, EIP712_NAME_HASH, EIP712_VERSION_HASH, block.chainid, address(this))
        );
    }

    function _hashTypedData(bytes32 structHash) internal view returns (bytes32) {
        return keccak256(abi.encodePacked(hex"1901", _domainSeparator(), structHash));
    }

    function _requireGovernanceSignature(bytes32 structHash, bytes calldata signature) internal view {
        address recovered = ECDSA.recoverCalldata(_hashTypedData(structHash), signature);
        require(recovered == governanceSigner, "bad governance signature");
    }

    function _validateMaxEpochDelta(uint256 value) internal view {
        if (!_isLocalDevelopmentChain()) require(value > 0, "max epoch delta=0");
    }

    /// @notice Preserve independent governance authority across later owner rotations.
    function transferOwnership(address newOwner) public override onlyOwner {
        require(newOwner != governanceSigner, "owner=governance signer");
        super.transferOwnership(newOwner);
    }

    /// @notice A live escrow must always retain an owner capable of posting the next governed root.
    function renounceOwnership() public view override onlyOwner {
        revert("owner required");
    }

    /// @notice Propose a cap change with independent governance authorization.
    function proposeMaxEpochDelta(uint256 newMax, bytes calldata governanceSignature) external onlyOwner {
        require(pendingMaxEpochDelta.executeAfter == 0, "max epoch delta pending");
        require(pendingRoot.executeAfter == 0, "root pending");
        require(newMax != maxEpochDelta, "max epoch delta unchanged");
        _validateMaxEpochDelta(newMax);
        uint256 nonce = governanceNonce;
        _requireGovernanceSignature(
            keccak256(abi.encode(MAX_EPOCH_DELTA_PROPOSAL_TYPEHASH, newMax, nonce)),
            governanceSignature
        );
        governanceNonce = nonce + 1;
        uint64 executeAfter = _executeAfter();
        pendingMaxEpochDelta = PendingMaxEpochDelta(newMax, nonce, executeAfter);
        emit MaxEpochDeltaProposed(newMax, nonce, executeAfter);
    }

    /// @notice Execute the already cross-signed cap change after its delay. Permissionless.
    function executeMaxEpochDelta() external {
        PendingMaxEpochDelta memory proposal = pendingMaxEpochDelta;
        require(proposal.executeAfter != 0, "no max epoch delta pending");
        require(block.timestamp >= proposal.executeAfter, "governance delay");
        require(pendingRoot.executeAfter == 0, "root pending");
        _validateMaxEpochDelta(proposal.newMaxEpochDelta);
        delete pendingMaxEpochDelta;
        maxEpochDelta = proposal.newMaxEpochDelta;
        emit MaxEpochDeltaSet(proposal.newMaxEpochDelta);
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

    // -- cross-signed, delayed cumulative distribution root -----------------------------------------
    function _validateRoot(bytes32 newRoot, uint256 newEpoch, uint256 newCumulativeSpent) internal view {
        require(newRoot != bytes32(0), "root=0");
        require(newEpoch > epoch, "epoch !monotonic");
        require(newCumulativeSpent >= cumulativeSpent, "spent !monotonic");
        require(newCumulativeSpent <= totalDeposited, "spent > deposited");
        if (maxEpochDelta > 0) {
            require(newCumulativeSpent - cumulativeSpent <= maxEpochDelta, "epoch delta > cap");
        }
    }

    /// @notice Owner proposes a root carrying an independent EIP-712 signature. No value moves yet.
    function proposeRoot(
        bytes32 newRoot,
        uint256 newEpoch,
        uint256 newCumulativeSpent,
        bytes calldata governanceSignature
    ) external onlyOwner {
        require(pendingRoot.executeAfter == 0, "root pending");
        require(pendingMaxEpochDelta.executeAfter == 0, "max epoch delta pending");
        _validateRoot(newRoot, newEpoch, newCumulativeSpent);
        uint256 nonce = governanceNonce;
        _requireGovernanceSignature(
            keccak256(
                abi.encode(
                    ROOT_PROPOSAL_TYPEHASH,
                    newRoot,
                    newEpoch,
                    newCumulativeSpent,
                    epoch,
                    cumulativeSpent,
                    nonce
                )
            ),
            governanceSignature
        );
        governanceNonce = nonce + 1;
        uint64 executeAfter = _executeAfter();
        pendingRoot = PendingRoot({
            merkleRoot: newRoot,
            newEpoch: newEpoch,
            newCumulativeSpent: newCumulativeSpent,
            previousEpoch: epoch,
            previousCumulativeSpent: cumulativeSpent,
            nonce: nonce,
            executeAfter: executeAfter
        });
        emit RootProposed(newEpoch, newRoot, newCumulativeSpent, nonce, executeAfter);
    }

    /// @notice Execute the cross-signed root after the immutable delay. Permissionless.
    function executeRoot() external {
        PendingRoot memory proposal = pendingRoot;
        require(proposal.executeAfter != 0, "no root pending");
        require(block.timestamp >= proposal.executeAfter, "governance delay");
        require(epoch == proposal.previousEpoch, "root base epoch changed");
        require(cumulativeSpent == proposal.previousCumulativeSpent, "root base spend changed");
        _validateRoot(proposal.merkleRoot, proposal.newEpoch, proposal.newCumulativeSpent);
        delete pendingRoot;
        merkleRoot = proposal.merkleRoot;
        merkleRootAtEpoch[proposal.newEpoch] = proposal.merkleRoot;
        epoch = proposal.newEpoch;
        cumulativeSpent = proposal.newCumulativeSpent;
        emit RootPosted(proposal.newEpoch, proposal.merkleRoot, proposal.newCumulativeSpent, proposal.nonce);
    }

    // -- O(1) claim - providers' 75% earnings AND buyer refunds (same root) ------------------------
    /// @notice Claim the delta of `cumulativeAmount` over what `account` has already claimed.
    ///         Permissionless (anyone may submit; funds always go to `account`). O(1) - no iteration.
    /// @dev Leaf uses the OZ StandardMerkleTree double-hash encoding:
    ///      keccak256(bytes.concat(keccak256(abi.encode(account, cumulativeAmount)))).
    function claim(
        uint256 rootEpoch,
        address account,
        uint256 cumulativeAmount,
        bytes32[] calldata proof
    ) external nonReentrant {
        // Guard: never pay the zero address or the pool itself (a malformed root leaf can't strand/burn funds).
        require(account != address(0) && account != address(this), "bad account");
        bytes32 claimRoot = merkleRootAtEpoch[rootEpoch];
        require(claimRoot != bytes32(0), "unknown root epoch");
        bytes32 leaf = keccak256(bytes.concat(keccak256(abi.encode(account, cumulativeAmount))));
        require(MerkleProof.verify(proof, claimRoot, leaf), "bad proof");

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
        emit Claimed(rootEpoch, account, cumulativeAmount, delta);
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
    /// @notice Still-burnable share. The provider and operator legs floor; their sub-wei split remainder is
    ///         assigned to the burn leg so every wei of cumulative spend has a deterministic destination.
    function burnClaimable() public view returns (uint256) {
        uint256 providerCap = (cumulativeSpent * PROVIDER_BPS) / BPS;
        uint256 operatorCap = (cumulativeSpent * OPERATOR_BPS) / BPS;
        uint256 cap = cumulativeSpent - providerCap - operatorCap;
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

    /// @notice Propose recovery of surplus with the same cross-signature and delay as a root.
    function proposeRescue(address to, uint256 amount, bytes calldata governanceSignature) external onlyOwner {
        require(pendingRescue.executeAfter == 0, "rescue pending");
        require(to != address(0), "to=0");
        require(amount > 0 && amount <= rescuableSurplus(), "exceeds surplus");
        uint256 nonce = governanceNonce;
        _requireGovernanceSignature(
            keccak256(abi.encode(RESCUE_PROPOSAL_TYPEHASH, to, amount, nonce)),
            governanceSignature
        );
        governanceNonce = nonce + 1;
        uint64 executeAfter = _executeAfter();
        pendingRescue = PendingRescue(to, amount, nonce, executeAfter);
        emit RescueProposed(to, amount, nonce, executeAfter);
    }

    /// @notice Execute the delayed surplus rescue. Permissionless; escrow remains unreachable.
    function executeRescue() external nonReentrant {
        PendingRescue memory proposal = pendingRescue;
        require(proposal.executeAfter != 0, "no rescue pending");
        require(block.timestamp >= proposal.executeAfter, "governance delay");
        require(proposal.amount <= rescuableSurplus(), "exceeds surplus");
        delete pendingRescue;
        token.safeTransfer(proposal.to, proposal.amount);
        emit Rescued(proposal.to, proposal.amount);
    }

    // -- views (audits / non-custodial checks) -----------------------------------------------------
    /// @notice TAP currently escrowed. Invariant: poolBalance == totalDeposited - totalClaimed - operatorWithdrawn - totalBurned.
    function poolBalance() external view returns (uint256) {
        return token.balanceOf(address(this));
    }
}
