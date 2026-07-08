// MayhemInferencePool test suite (I2-B1) - deploy MockTTAP + MayhemInferencePool on in-process
// ganache; exercise deposit -> setRoot -> O(1) cumulative claim -> operator 15% cap -> conservation ->
// non-custodial property -> all the revert guards. Exit 0 = green. No external network.
import Ganache from 'ganache';
import { ethers } from 'ethers';
import { compileAll } from './compile.mjs';
import { distribution } from './merkle.mjs';

const CHAIN_ID = 61000;
let passed = 0;
const ok = (cond, label) => { if (!cond) { console.error(`  [fail] ${label}`); throw new Error('assert failed: ' + label); } console.log(`  [ok] ${label}`); passed++; };
async function expectRevert(promiseFn, fragment, label) {
  try { const tx = await promiseFn(); if (tx?.wait) await tx.wait(); }
  catch (e) {
    const m = [e.shortMessage, e.reason, e.revert?.args?.[0], e.info?.error?.message, e.message].filter(Boolean).join(' | ');
    // fragment '*' = assert it reverted at all (used where ganache's estimateGas can't return the
    // custom-error data to decode a specific OZ error name).
    const matched = fragment === '*' ? /revert/i.test(m) : m.includes(fragment);
    if (!matched) console.error(`    (actual revert: ${m})`);
    ok(matched, `${label} (revert: ${fragment})`);
    return;
  }
  ok(false, `${label} - expected revert "${fragment}" but it succeeded`);
}

async function main() {
  const art = compileAll();
  const ganache = Ganache.provider({ logging: { quiet: true }, chain: { chainId: CHAIN_ID }, wallet: { totalAccounts: 8 } });
  const provider = new ethers.BrowserProvider(ganache);
  const S = async (i) => provider.getSigner(i);
  const A = async (i) => (await S(i)).getAddress();

  const operator = await S(0);          // pool owner / root authority
  const [buyer1, buyer2, provA, provB, anyone, opTreasury] = [await A(1), await A(2), await A(3), await A(4), await A(5), await A(6)];

  const U = (n) => ethers.parseUnits(String(n), 18);
  const DEAD = ethers.getAddress('0x000000000000000000000000000000000000dead'); // burn sink (== MayhemInferencePool.BURN_SINK)

  // deploy MockTTAP + pool
  const token = await new ethers.ContractFactory(art.MockTTAP.abi, art.MockTTAP.bytecode, operator).deploy();
  await token.waitForDeployment();
  const pool = await new ethers.ContractFactory(art.MayhemInferencePool.abi, art.MayhemInferencePool.bytecode, operator).deploy(await token.getAddress(), await A(0), 0n);
  await pool.waitForDeployment();
  const poolAddr = await pool.getAddress();
  console.log(`MockTTAP @ ${await token.getAddress()} - MayhemInferencePool @ ${poolAddr}`);

  // fund buyers + approve
  for (const b of [1, 2]) { await (await token.mint(await A(b), U(1000))).wait(); await (await token.connect(await S(b)).approve(poolAddr, U(1000))).wait(); }

  console.log('\n1) deposits');
  await (await pool.connect(await S(1)).deposit(U(1000))).wait();
  await (await pool.connect(await S(2)).deposit(U(1000))).wait();
  ok((await pool.totalDeposited()) === U(2000), 'totalDeposited = 2000 after two deposits');
  ok((await pool.poolBalance()) === U(2000), 'pool escrows 2000 TAP');
  await expectRevert(async () => pool.connect(await S(1)).deposit.staticCall(0n), 'amount=0', 'deposit 0 reverts');

  // -- Epoch 1: spent 1000 -> providers 75% (A=450,B=300), operator cap 150, burn 100 -----------------
  console.log('\n2) epoch 1 - cumulative root {A:450, B:300}, spent 1000 (75/15/10)');
  const e1 = distribution([{ account: provA, amount: U(450) }, { account: provB, amount: U(300) }]);
  await (await pool.setRoot(e1.root, 1, U(1000))).wait();
  ok((await pool.epoch()) === 1n && (await pool.cumulativeSpent()) === U(1000), 'root posted, epoch=1, spent=1000');

  // non-custodial + permissionless: `anyone` submits provA's claim; funds go to provA, no operator action.
  await (await pool.connect(await S(5)).claim(provA, U(450), e1.proofFor(provA))).wait();
  ok((await token.balanceOf(provA)) === U(450), 'provA received 450 (claim submitted by a third party - non-custodial)');
  await (await pool.connect(await S(4)).claim(provB, U(300), e1.proofFor(provB))).wait();
  ok((await token.balanceOf(provB)) === U(300), 'provB received 300');
  ok((await pool.totalClaimed()) === U(750), 'totalClaimed = 750 (providers 75% of 1000)');

  await expectRevert(() => pool.claim.staticCall(provA, U(450), e1.proofFor(provA)), 'nothing to claim', 're-claim same cumulative reverts');
  await expectRevert(() => pool.claim.staticCall(provA, U(9999), e1.proofFor(provA)), 'bad proof', 'forged amount -> bad proof');

  // operator withdraws its 15% of spent (=150), then is capped
  await (await pool.withdrawOperator(opTreasury, U(150))).wait();
  ok((await token.balanceOf(opTreasury)) === U(150), 'operator withdrew 150 (15% of spent 1000)');
  ok((await pool.operatorClaimable()) === 0n, 'operatorClaimable now 0');
  await expectRevert(() => pool.withdrawOperator.staticCall(opTreasury, U(1)), 'exceeds 15% cap', 'operator over-cap reverts');

  // -- 10% burn - PERMISSIONLESS: a non-owner fires it; 100 TAP -> dead address (deflationary) ---------
  ok((await pool.burnClaimable()) === U(100), 'burnClaimable = 100 (10% of spent 1000)');
  await (await pool.connect(await S(5)).burn()).wait();   // S(5)=anyone, NOT the owner - permissionless
  ok((await token.balanceOf(DEAD)) === U(100), 'dead address received 100 burned TAP (provably out of circulation)');
  ok((await pool.totalBurned()) === U(100), 'totalBurned = 100');
  ok((await pool.burnClaimable()) === 0n, 'burnClaimable now 0 (fully burned for this epoch)');
  await expectRevert(() => pool.burn.staticCall(), 'nothing to burn', 're-burn with nothing accrued reverts');

  // root-authority guards (staticCall = eth_call -> revert data returned reliably + custom errors decode)
  await expectRevert(() => pool.setRoot.staticCall(e1.root, 1, U(1000)), 'epoch !monotonic', 'non-monotonic epoch reverts');
  await expectRevert(() => pool.setRoot.staticCall(e1.root, 2, U(3000)), 'spent > deposited', 'spent > deposited reverts');
  await expectRevert(async () => pool.connect(await S(1)).setRoot.staticCall(e1.root, 2, U(1200)), 'OwnableUnauthorizedAccount', 'non-owner setRoot reverts');

  // -- Epoch 2: cumulative - A=810,B=540 (providers 75% of 1800), buyer2 refund 200, op cap 270, burn 180
  console.log('\n3) epoch 2 - cumulative root {A:810, B:540, buyer2:200}, spent 1800 (75/15/10)');
  const e2 = distribution([{ account: provA, amount: U(810) }, { account: provB, amount: U(540) }, { account: buyer2, amount: U(200) }]);
  await (await pool.setRoot(e2.root, 2, U(1800))).wait();

  await (await pool.claim(provA, U(810), e2.proofFor(provA))).wait();
  ok((await token.balanceOf(provA)) === U(810), 'provA cumulative 810 (delta 360 paid)');
  await (await pool.claim(provB, U(540), e2.proofFor(provB))).wait();
  ok((await token.balanceOf(provB)) === U(540), 'provB cumulative 540 (delta 240 paid)');
  await (await pool.claim(buyer2, U(200), e2.proofFor(buyer2))).wait();
  ok((await token.balanceOf(buyer2)) === U(200), 'buyer2 refund 200 claimed (same root, same claim path)');

  await (await pool.withdrawOperator(opTreasury, U(120))).wait();
  ok((await token.balanceOf(opTreasury)) === U(270), 'operator cumulative 270 (15% of 1800)');

  // burn the new 10% delta (cap now 180; 100 already burned -> 80 newly burnable)
  ok((await pool.burnClaimable()) === U(80), 'burnClaimable = 80 (cap 180 - 100 already burned)');
  await (await pool.connect(await S(4)).burn()).wait();   // again permissionless (provB's signer fires it)
  ok((await token.balanceOf(DEAD)) === U(180), 'dead address cumulative 180 burned (10% of 1800)');
  ok((await pool.totalBurned()) === U(180), 'totalBurned cumulative 180');

  // -- conservation + full-settlement drain ---------------------------------------------------------
  console.log('\n4) conservation (three sinks: provider/refund claims + operator 15% + burn 10%)');
  const totalClaimed = await pool.totalClaimed();
  const operatorWithdrawn = await pool.operatorWithdrawn();
  const totalBurned = await pool.totalBurned();
  const totalDeposited = await pool.totalDeposited();
  ok(totalClaimed === U(1550), 'totalClaimed = 1550 (providers 1350 + buyer2 refund 200)');
  ok(operatorWithdrawn === U(270), 'operatorWithdrawn = 270 (15% of 1800)');
  ok(totalBurned === U(180), 'totalBurned = 180 (10% of 1800)');
  ok(totalClaimed + operatorWithdrawn + totalBurned === totalDeposited, 'sum claims + operator + burn == deposits (conservation, exact at full settlement)');
  ok((await pool.poolBalance()) === 0n, 'pool fully drained to 0');
  ok((await pool.poolBalance()) === totalDeposited - totalClaimed - operatorWithdrawn - totalBurned, 'poolBalance == deposited - claimed - operator - burned (invariant)');

  // -- C1 (per-epoch cap + zero-addr guard) / M11 (measured-delta) / rescue (surplus) ----------------
  console.log('\n5) C1 cap + zero-addr guard + M11 measured-delta + rescue (security remediation)');
  const ZERO = '0x0000000000000000000000000000000000000000';
  const cap = U(100);
  const pool2 = await new ethers.ContractFactory(art.MayhemInferencePool.abi, art.MayhemInferencePool.bytecode, operator).deploy(await token.getAddress(), await A(0), cap);
  await pool2.waitForDeployment();
  const pool2Addr = await pool2.getAddress();
  ok((await pool2.maxEpochDelta()) === cap, 'maxEpochDelta set in constructor (C1)');

  await (await token.mint(await A(1), U(1000))).wait();
  await (await token.connect(await S(1)).approve(pool2Addr, U(1000))).wait();
  await (await pool2.connect(await S(1)).deposit(U(1000))).wait();
  ok((await pool2.totalDeposited()) === U(1000), 'M11: deposit credits the MEASURED amount (1000)');

  const r1 = distribution([{ account: provA, amount: U(50) }]);
  await expectRevert(() => pool2.setRoot.staticCall(r1.root, 1, U(200)), 'epoch delta > cap', 'C1: setRoot whose new spend (200) exceeds the per-epoch cap (100) reverts');
  await (await pool2.setRoot(r1.root, 1, U(100))).wait();
  ok((await pool2.cumulativeSpent()) === U(100), 'C1: a setRoot at exactly the cap (delta 100) is allowed');

  const rz = distribution([{ account: ZERO, amount: U(10) }, { account: provA, amount: U(20) }]);
  await (await pool2.setRoot(rz.root, 2, U(100))).wait(); // delta 0, within cap
  await expectRevert(() => pool2.claim.staticCall(ZERO, U(10), rz.proofFor(ZERO)), 'bad account', 'C1: a claim to address(0) reverts (zero/self guard)');

  await expectRevert(async () => pool2.connect(await S(1)).setMaxEpochDelta.staticCall(U(1)), 'OwnableUnauthorizedAccount', 'setMaxEpochDelta is owner-gated');

  await (await token.mint(pool2Addr, U(7))).wait(); // stray transfer NOT via deposit -> non-accounted surplus
  ok((await pool2.rescuableSurplus()) === U(7), 'rescue: a stray 7 TAP transfer is rescuable surplus');
  await expectRevert(() => pool2.rescue.staticCall(opTreasury, U(8)), 'exceeds surplus', 'rescue > surplus reverts (escrow protected)');
  await (await pool2.rescue(opTreasury, U(7))).wait();
  ok((await pool2.rescuableSurplus()) === 0n, 'rescue: surplus swept; the 1000 escrow is untouched');
  ok((await pool2.totalDeposited()) === U(1000), 'rescue never touched accounted escrow');

  console.log(`\nMAYHEM TAP POOL TEST OK - ${passed} checks passed`);
  process.exit(0);
}

main().catch((e) => { console.error('\nMAYHEM TAP POOL TEST FAILED:', e.message); process.exit(1); });
