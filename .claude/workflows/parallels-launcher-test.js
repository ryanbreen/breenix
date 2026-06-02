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
  },
  required: ['pass', 'reason', 'evidencePath'],
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
  '',
  'Never report pass=true on "launcher opened" or "process created" alone — only on the exact "RESULT: PASS" line.',
  'Do NOT run multiple VMs in parallel; this single run owns the one Parallels VM. Do NOT modify any files.',
].join('\n');

phase('Gate');

let consecutive = 0;
let greenStreakMax = 0;
let attempts = 0;
let firstFailure = null;
let lastEvidenceDir = '';
const perAttempt = [];

for (let i = 1; i <= MAX_ATTEMPTS; i++) {
  attempts = i;
  log('Attempt ' + i + '/' + MAX_ATTEMPTS + ' — consecutive-green streak: ' + consecutive + '/' + TARGET_STREAK);

  const result = await agent(ATTEMPT_PROMPT, { schema: ATTEMPT_SCHEMA, label: 'attempt-' + i, phase: 'Gate' });

  const r = result || { pass: false, reason: 'agent returned null', injectionMs: -1, launcherOpened: false, evidencePath: '' };
  perAttempt.push({ attempt: i, pass: r.pass, reason: r.reason, injectionMs: r.injectionMs, launcherOpened: r.launcherOpened });
  if (r.evidencePath) {
    lastEvidenceDir = r.evidencePath;
  }

  if (r.pass) {
    consecutive = consecutive + 1;
    if (consecutive > greenStreakMax) {
      greenStreakMax = consecutive;
    }
    log('Attempt ' + i + ' PASS — streak now ' + consecutive + '/' + TARGET_STREAK + ' (inject ' + r.injectionMs + 'ms)');
    if (consecutive >= TARGET_STREAK) {
      log('Reached ' + TARGET_STREAK + ' consecutive green; stopping after ' + i + ' attempts.');
      break;
    }
  } else {
    if (firstFailure === null) {
      firstFailure = { attempt: i, reason: r.reason, injectionMs: r.injectionMs, launcherOpened: r.launcherOpened, evidencePath: r.evidencePath };
    }
    log('Attempt ' + i + ' FAIL (' + r.reason + ') — streak reset ' + consecutive + ' -> 0; continuing to measure flakiness.');
    consecutive = 0;
  }
}

const consecutiveGreenAchieved = greenStreakMax >= TARGET_STREAK;
log('Done. attempts=' + attempts + ' greenStreakMax=' + greenStreakMax + ' consecutiveGreenAchieved=' + consecutiveGreenAchieved);

return {
  consecutiveGreenAchieved: consecutiveGreenAchieved,
  greenStreakMax: greenStreakMax,
  attempts: attempts,
  firstFailure: firstFailure,
  perAttempt: perAttempt,
  evidenceDir: lastEvidenceDir,
};
