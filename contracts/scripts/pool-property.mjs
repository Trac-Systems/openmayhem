// Mayhem TAP pool PROPERTY/FUZZ test (I2-B1 - money-safety invariants under randomized
// settlement). A seeded PRNG drives many epochs of cumulative roots (monotonic spent, monotonic
// per-provider owed <= 75% of spent) with interleaved claims + operator withdrawals + permissionless burns;
// after EVERY action the on-chain invariants must hold: CONSERVATION (poolBalance == deposited - claimed -
// operator - burned), 15% OPERATOR CAP, 10% BURN CAP, claimed[acct] never exceeds rooted cumulative. Attacks:
// double-claim, claim-beyond-owed (forged amount), wrong-account proof, and over-cap withdraw all
// REVERT. Uses the SAME merkle builder the engine settlement roller posts. Run: `npm run pool:property`.
import Ganache from 'ganache';
import { ethers } from 'ethers';
import { deployPool } from './deploy-local.mjs';
import { distribution } from './merkle.mjs';

const EPOCHS = 12;
const W = [50n, 30n, 20n]; // provider split (sums to 100) - fixed -> cumulative owed stays monotonic
function rng(seed) { let s = seed >>> 0; return () => (s = (s * 1664525 + 1013904223) >>> 0) / 0x100000000; }
const r = rng(0x5EED5);

let fails = 0;
const ok = (c, label) => { if (!c) { console.log(`  [fail] ${label}`); fails++; } };
async function expectRevert(thunk, label) {
  try { await thunk(); ok(false, `${label} (did NOT revert)`); }
  catch { /* expected */ }
}

try {
  const provider = new ethers.BrowserProvider(Ganache.provider({ logging: { quiet: true }, chain: { chainId: 61000 }, wallet: { totalAccounts: 6 } }));
  const operator = await provider.getSigner(0);
  const operatorAddr = await operator.getAddress();
  const buyer = await provider.getSigner(1);
  const provs = [await provider.getSigner(2), await provider.getSigner(3), await provider.getSigner(4)];
  const provAddr = await Promise.all(provs.map((s) => s.getAddress()));
  const { token, pool, poolAddr } = await deployPool(operator);

  const DEP = ethers.parseUnits('1000000', 18);
  await (await token.mint(await buyer.getAddress(), DEP)).wait();
  await (await token.connect(buyer).approve(poolAddr, DEP)).wait();
  await (await pool.connect(buyer).deposit(DEP)).wait();

  const DEAD = ethers.getAddress('0x000000000000000000000000000000000000dead'); // burn sink (== BURN_SINK)
  let spent = 0n;
  const cum = [0n, 0n, 0n];           // last rooted cumulative per provider
  const claimedLocal = [0n, 0n, 0n];  // what we've claimed per provider
  let opWithdrawn = 0n;
  let burned = 0n;                    // sum TAP sent to the dead address via the permissionless burn()
  let lastDist = null;

  const assertInvariants = async (where) => {
    const deposited = await pool.totalDeposited();
    const tClaimed = BigInt(claimedLocal[0] + claimedLocal[1] + claimedLocal[2]);
    const poolBal = await pool.poolBalance();
    ok(poolBal === deposited - tClaimed - opWithdrawn - burned, `conservation @ ${where} (pool=${poolBal})`);
    const opCap = (spent * 1500n) / 10000n;
    ok((await pool.operatorClaimable()) === opCap - opWithdrawn, `operator claimable = 15% cap - withdrawn @ ${where}`);
    ok(opWithdrawn <= opCap, `operator withdrawn within 15% cap @ ${where}`);
    const burnCap = (spent * 1000n) / 10000n;
    ok((await pool.burnClaimable()) === burnCap - burned, `burn claimable = 10% cap - burned @ ${where}`);
    ok(burned <= burnCap, `burned within 10% cap @ ${where}`);
    for (let i = 0; i < 3; i++) ok((await pool.claimed(provAddr[i])) <= cum[i], `claimed <= rooted cumulative (prov ${i}) @ ${where}`);
  };

  for (let e = 1; e <= EPOCHS; e++) {
    // grow spent monotonically, staying <= deposited
    const room = DEP - spent;
    spent += BigInt(Math.floor(r() * Number(room / BigInt(EPOCHS))));
    const cap75 = (spent * 75n) / 100n;                          // providers <= 75% (leaves 15% op + 10% burn)
    for (let i = 0; i < 3; i++) cum[i] = (cap75 * W[i]) / 100n; // monotonic (spent grows)

    const entries = provAddr.map((a, i) => ({ account: a.toLowerCase(), amount: cum[i] })).filter((x) => x.amount > 0n);
    if (entries.length === 0) continue;
    lastDist = distribution(entries);
    await (await pool.connect(operator).setRoot(lastDist.root, e, spent)).wait();
    await assertInvariants(`epoch ${e} setRoot`);

    // each provider claims its current cumulative ~70% of the time
    for (let i = 0; i < 3; i++) {
      if (cum[i] > claimedLocal[i] && r() < 0.7) {
        await (await pool.connect(provs[i]).claim(provAddr[i], cum[i], lastDist.proofFor(provAddr[i]))).wait();
        claimedLocal[i] = cum[i];
        ok((await token.balanceOf(provAddr[i])) === claimedLocal[i], `prov ${i} on-chain balance == claimed cumulative`);
      }
    }
    // operator withdraws a random slice of its claimable ~40% of the time
    const claimable = (spent * 1500n) / 10000n - opWithdrawn;
    if (claimable > 0n && r() < 0.4) {
      const amt = BigInt(Math.max(1, Math.floor(r() * Number(claimable))));
      await (await pool.connect(operator).withdrawOperator(operatorAddr, amt)).wait();
      opWithdrawn += amt;
    }
    // ~50% of the time, a NON-owner fires the permissionless burn() for the accrued 10%
    const burnable = (spent * 1000n) / 10000n - burned;
    if (burnable > 0n && r() < 0.5) {
      await (await pool.connect(provs[0]).burn()).wait();        // permissionless - a provider, not the owner
      burned += burnable;
      ok((await token.balanceOf(DEAD)) === burned, `dead-address balance == cumulative burned @ epoch ${e}`);
    }
    await assertInvariants(`epoch ${e} post-claims`);
  }
  console.log(`  [ok] ${EPOCHS} fuzzed epochs: conservation + 15% cap + claimed<=cumulative held throughout`);

  // -- adversarial: these MUST revert -------------------------------------------------------------
  // double-claim the same cumulative (after a fresh claim -> nothing new)
  const i0 = 0;
  if (claimedLocal[i0] === cum[i0] && cum[i0] > 0n) {
    await expectRevert(() => pool.connect(provs[i0]).claim.staticCall(provAddr[i0], cum[i0], lastDist.proofFor(provAddr[i0])), 'double-claim same cumulative reverts');
  }
  // claim-beyond-owed: forge a higher amount with the real proof -> bad proof
  await expectRevert(() => pool.connect(provs[i0]).claim.staticCall(provAddr[i0], cum[i0] + 1n, lastDist.proofFor(provAddr[i0])), 'claim-beyond-owed (forged amount) reverts');
  // wrong-account proof: claim prov0's amount with prov1's proof
  await expectRevert(() => pool.connect(provs[i0]).claim.staticCall(provAddr[i0], cum[i0], lastDist.proofFor(provAddr[1])), 'mismatched proof reverts');
  // operator over-cap withdraw
  const overCap = (spent * 1500n) / 10000n - opWithdrawn + 1n;
  await expectRevert(() => pool.connect(operator).withdrawOperator.staticCall(operatorAddr, overCap), 'operator over-15%-cap withdraw reverts');
  // non-owner setRoot
  await expectRevert(() => pool.connect(provs[0]).setRoot.staticCall(lastDist.root, EPOCHS + 1, spent), 'non-owner setRoot reverts');
  // burn the remaining accrued 10%, then a re-burn with nothing accrued must revert (cap exhausted)
  if ((await pool.burnClaimable()) > 0n) { await (await pool.connect(provs[0]).burn()).wait(); burned = (spent * 1000n) / 10000n; }
  await expectRevert(() => pool.connect(provs[0]).burn.staticCall(), 'burn with nothing accrued reverts');
  ok((await token.balanceOf(DEAD)) === burned, 'dead-address holds exactly the cumulative burn (10% of spent)');
  if (fails === 0) console.log('  [ok] adversarial: double-claim / forged-amount / wrong-proof / over-cap / non-owner / over-burn all revert');

  console.log(`\n${fails === 0 ? 'MAYHEM TAP POOL PROPERTY OK - invariants held across fuzzed settlement + all attacks reverted' : `MAYHEM TAP POOL PROPERTY: ${fails} check(s) failed`}`);
  process.exit(fails === 0 ? 0 : 1);
} catch (e) {
  console.error('MAYHEM TAP POOL PROPERTY FAILED:', e?.stack || e?.message || e);
  process.exit(1);
}
