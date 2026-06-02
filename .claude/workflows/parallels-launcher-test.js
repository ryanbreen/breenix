export const meta = {
  name: 'parallels-launcher-test',
  description: 'Drive the Breenix launcher->terminal smoke test on a fresh Parallels VM, sequentially (one VM, never parallel), measuring the consecutive-green streak until 10-in-a-row or 15 attempts.',
  phases: [
    { name: 'run-smoke-attempts', description: 'Run launcher-smoke.sh up to 15 times sequentially; stop early at a 10-consecutive-PASS streak.' },
  ],
};

const MAX_ATTEMPTS = 15;
const TARGET_STREAK = 10;

const attemptSchema = {
  type: 'object',
  properties: {
    pass: { type: 'boolean', description: 'true only if the script printed exactly "RESULT: PASS"' },
    reason: { type: 'string', description: 'For a FAIL, the reason after "RESULT: FAIL:"; for a PASS, "ok".' },
    evidencePath: { type: 'string', description: 'Absolute path to the run-* evidence dir created by this attempt (from result.txt evidence_dir=), or empty string if none.' },
  },
  required: ['pass', 'reason', 'evidencePath'],
  additionalProperties: false,
};

export default async function run() {
  let consecutive = 0;
  let greenStreakMax = 0;
  let attempts = 0;
  let firstFailure = null;
  let lastEvidenceDir = '';

  for (let i = 1; i <= MAX_ATTEMPTS; i++) {
    attempts = i;
    log('Attempt ' + i + '/' + MAX_ATTEMPTS + ' — current consecutive-green streak: ' + consecutive + ' (target ' + TARGET_STREAK + ')');

    const result = await agent({
      schema: attemptSchema,
      prompt: [
        'Run the Breenix launcher->terminal smoke test ONCE and report the structured outcome.',
        '',
        'HOW TO RUN (mandatory):',
        '- Use the Bash tool with dangerouslyDisableSandbox set to true and run_in_background set to true.',
        '- Command: bash /Users/wrb/fun/code/breenix/scripts/parallels/launcher-smoke.sh',
        '- A single run takes roughly 8-15 minutes (full VM boot + VirGL warmup + injection).',
        '- Because it is backgrounded, poll its output periodically until it prints a line that begins with "RESULT:".',
        '  Do NOT give up early; wait for the RESULT line or for the process to exit.',
        '',
        'PARSING THE OUTCOME (be strictly honest):',
        '- pass = true ONLY if the final line is exactly "RESULT: PASS".',
        '- If the final line is "RESULT: FAIL: <reason>", set pass = false and reason = the text after "RESULT: FAIL:".',
        '- If the script never prints a RESULT line (e.g. it crashed or was killed), set pass = false and reason = "no RESULT line emitted".',
        '- evidencePath = the value of "evidence_dir=" in the run\'s result.txt (the script prints the evidence dir; it is under',
        '  /Users/wrb/fun/code/breenix/logs/parallels-launcher-test/run-<timestamp>/). If you cannot determine it, use an empty string.',
        '',
        'Never report pass = true based on "launcher opened" or "process created" alone — only on the exact "RESULT: PASS" line.',
        'Do NOT run multiple VMs in parallel; this single run owns the one Parallels VM.',
      ].join('\n'),
    });

    if (result.evidencePath) {
      lastEvidenceDir = result.evidencePath;
    }

    if (result.pass) {
      consecutive = consecutive + 1;
      if (consecutive > greenStreakMax) {
        greenStreakMax = consecutive;
      }
      log('Attempt ' + i + ' PASS — consecutive streak now ' + consecutive + '/' + TARGET_STREAK);
      if (consecutive >= TARGET_STREAK) {
        log('Reached ' + TARGET_STREAK + ' consecutive green; stopping early after ' + i + ' attempts.');
        break;
      }
    } else {
      if (firstFailure === null) {
        firstFailure = { attempt: i, reason: result.reason, evidencePath: result.evidencePath };
      }
      log('Attempt ' + i + ' FAIL (' + result.reason + ') — streak reset from ' + consecutive + ' to 0; continuing to measure flakiness.');
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
    evidenceDir: lastEvidenceDir,
  };
}
