export const meta = {
  name: 'parallels-launcher-test',
  description: 'Drive the Breenix launcher->terminal smoke test on a fresh Parallels VM, sequentially (one VM, never parallel), measuring the consecutive-green streak until 10-in-a-row or 15 attempts.',
  phases: [
    { title: 'Gate', detail: 'Run launcher-smoke.sh --no-build up to 15 times sequentially; stop early at a 10-consecutive-PASS streak.' },
  ],
};

const MAX_ATTEMPTS = 15;
const TARGET_STREAK = 10;

const ATTEMPT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    pass: { type: 'boolean', description: 'true ONLY if the script printed exactly "RESULT: PASS"' },
    reason: { type: 'string', description: 'For a FAIL, the text after "RESULT: FAIL:"; for a PASS, "ok".' },
    injectionMs: { type: 'integer', description: 'The double-tap injection wall-time in ms from the smoke log line "double-tap injection wall-time: <N>ms", or -1 if not found.' },
    launcherOpened: { type: 'boolean', description: 'true if the serial/evidence shows the launcher opened this run.' },
    evidencePath: { type: 'string', description: 'Absolute path to the run-* evidence dir (from result.txt evidence_dir=), or empty string.' },
    injectRetries: { type: 'integer', description: 'The integer from result.txt\'s "inject_retries=<N>" line (result.txt lives directly in evidence_dir). 0 = the launcher opened on the very first double-tap, no retry. >0 = it took N extra re-injections to recover -- a PASS with this is NOT a clean pass, it is masking an input-drop. -1 if result.txt is missing or the field cannot be found; NEVER guess.' },
  },
  required: ['pass', 'reason', 'evidencePath', 'injectRetries'],
};

const ATTEMPT_PROMPT = [
  'Run the Breenix launcher->terminal smoke test ONCE and report the structured outcome.',
  '',
  'HOW TO RUN (mandatory):',
  '- Use the Bash tool with dangerouslyDisableSandbox:true AND run_in_background:true.',
  '- Command (note --no-build: artifacts already exist; a per-run rebuild is wrong and wasteful):',
  '    bash /Users/wrb/fun/code/breenix/scripts/parallels/launcher-smoke.sh --no-build',
  '- A single run takes ~6-10 min (fresh VM boot + ~60s VirGL warmup + injection + validation).',
  '- Because it is backgrounded, poll its output every ~30s until it prints a line beginning with "RESULT:".',
  '  Do NOT give up early; wait for the RESULT line or for the process to exit (allow up to ~22 min).',
  '',
  'BEFORE running, confirm the macOS screen is UNLOCKED:',
  '  python3 -c "import Quartz;d=Quartz.CGSessionCopyCurrentDictionary();print(\'LOCKED\' if (d and d.get(\'CGSSessionScreenIsLocked\')) else \'UNLOCKED\')"',
  '  If it prints LOCKED, do NOT run; return pass=false, reason="aborted: macOS screen is locked (Parallels drops injected keys)".',
  '',
  'PARSING THE OUTCOME (be strictly honest):',
  '- pass = true ONLY if the final line is exactly "RESULT: PASS".',
  '- If "RESULT: FAIL: <reason>", pass=false and reason = the text after "RESULT: FAIL:".',
  '- If no RESULT line is ever printed, pass=false and reason="no RESULT line emitted".',
  '- injectionMs = the integer from the smoke log line "double-tap injection wall-time: <N>ms" (look in the backgrounded output / the run dir); -1 if not found. (>350ms means the double-tap likely missed its 400ms window.)',
  '- launcherOpened = true if the run evidence/serial shows the launcher opened (e.g. grep the run dir / serial for "[spawn] path=\'/bin/blauncher\'").',
  '- evidencePath = the "evidence_dir=" value from the run\'s result.txt (under /Users/wrb/fun/code/breenix/logs/parallels-launcher-test/run-<ts>/); empty string if unknown.',
  '- injectRetries = cat <evidencePath>/result.txt and read the "inject_retries=<N>" line. 0 means the launcher opened on the very first double-tap (a genuinely clean pass). A PASS with injectRetries>0 means the harness had to re-inject to recover from a missed double-tap -- report it honestly as pass=true with the real injectRetries value; do NOT round it down to 0. -1 ONLY if result.txt truly has no such line.',
  '',
  'Never report pass=true on "launcher opened" or "process created" alone — only on the exact "RESULT: PASS" line.',
  'Never fudge injectRetries to make a run look cleaner than it was — a retried PASS is still reported honestly (it just will not count toward the clean-streak gate).',
  'Do NOT run multiple VMs in parallel; this single run owns the one Parallels VM. Do NOT modify any files.',
].join('\n');

phase('Gate');

// Two streaks are tracked, deliberately kept separate:
//   - `consecutive` / `greenStreakMax` — the RAW pass streak (any "RESULT: PASS",
//     including one recovered via the harness's inject-retry budget). Informational
//     only — a retried pass is real evidence the harness recovered, but it is NOT
//     proof the double-tap landed cleanly, so it must never drive the gate.
//   - `cleanConsecutive` / `cleanStreakMax` — passes with injectRetries === 0 ONLY.
//     This is the actual 10x gate: a PASS achieved only after re-injecting masks an
//     input-drop regression, so it does not count toward — and resets — this streak.
let consecutive = 0;
let greenStreakMax = 0;
let cleanConsecutive = 0;
let cleanStreakMax = 0;
let attempts = 0;
let firstFailure = null;
let lastEvidenceDir = '';
const perAttempt = [];

for (let i = 1; i <= MAX_ATTEMPTS; i++) {
  attempts = i;
  log('Attempt ' + i + '/' + MAX_ATTEMPTS + ' — clean-green streak: ' + cleanConsecutive + '/' + TARGET_STREAK + ' (raw pass streak: ' + consecutive + ')');

  const result = await agent(ATTEMPT_PROMPT, { schema: ATTEMPT_SCHEMA, label: 'attempt-' + i, phase: 'Gate' });

  const r = result || { pass: false, reason: 'agent returned null', injectionMs: -1, launcherOpened: false, evidencePath: '', injectRetries: -1 };
  // injectRetries === 0 is the ONLY value that counts as a clean pass. Anything
  // else (>0, or -1/unknown because it couldn't be parsed) is treated
  // conservatively as NOT clean — we never assume clean without positive proof.
  const injectRetries = typeof r.injectRetries === 'number' ? r.injectRetries : -1;
  const clean = r.pass === true && injectRetries === 0;
  // Raw pass data is recorded honestly regardless of streak bookkeeping.
  perAttempt.push({ attempt: i, pass: r.pass, reason: r.reason, injectionMs: r.injectionMs, launcherOpened: r.launcherOpened, injectRetries: injectRetries, clean: clean });
  if (r.evidencePath) {
    lastEvidenceDir = r.evidencePath;
  }

  if (r.pass) {
    consecutive = consecutive + 1;
    if (consecutive > greenStreakMax) {
      greenStreakMax = consecutive;
    }
    if (clean) {
      cleanConsecutive = cleanConsecutive + 1;
      if (cleanConsecutive > cleanStreakMax) {
        cleanStreakMax = cleanConsecutive;
      }
      log('Attempt ' + i + ' PASS (clean, inject_retries=0) — clean streak now ' + cleanConsecutive + '/' + TARGET_STREAK + ' (inject ' + r.injectionMs + 'ms)');
      if (cleanConsecutive >= TARGET_STREAK) {
        log('Reached ' + TARGET_STREAK + ' consecutive CLEAN green; stopping after ' + i + ' attempts.');
        break;
      }
    } else {
      log('Attempt ' + i + ' PASS but NOT clean (inject_retries=' + injectRetries + ') — masks an input-drop; does NOT count toward the clean-streak gate. Clean streak reset ' + cleanConsecutive + ' -> 0 (raw pass streak still ' + consecutive + ').');
      cleanConsecutive = 0;
    }
  } else {
    if (firstFailure === null) {
      firstFailure = { attempt: i, reason: r.reason, injectionMs: r.injectionMs, launcherOpened: r.launcherOpened, evidencePath: r.evidencePath, injectRetries: injectRetries };
    }
    log('Attempt ' + i + ' FAIL (' + r.reason + ') — raw streak reset ' + consecutive + ' -> 0, clean streak reset ' + cleanConsecutive + ' -> 0; continuing to measure flakiness.');
    consecutive = 0;
    cleanConsecutive = 0;
  }
}

const consecutiveGreenAchieved = cleanStreakMax >= TARGET_STREAK;
log('Done. attempts=' + attempts + ' cleanStreakMax=' + cleanStreakMax + ' greenStreakMax(raw)=' + greenStreakMax + ' consecutiveGreenAchieved=' + consecutiveGreenAchieved);

return {
  consecutiveGreenAchieved: consecutiveGreenAchieved,
  cleanStreakMax: cleanStreakMax,
  greenStreakMax: greenStreakMax,
  attempts: attempts,
  firstFailure: firstFailure,
  perAttempt: perAttempt,
  evidenceDir: lastEvidenceDir,
};
