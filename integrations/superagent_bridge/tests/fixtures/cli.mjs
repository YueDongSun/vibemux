// SYNTHETIC TEST EXECUTABLE. It cannot start workers or access any daemon.
import { appendFile, readFile, writeFile, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { taskPage, taskDetail, workflow, reports, WORKFLOW, MESSAGE, CONTRACT } from './data.mjs';

const [scenario, directory, ...args] = process.argv.slice(2);
const receiptFile = path.join(directory, 'receipt.json');
const journal = path.join(directory, 'calls.jsonl');
const print = (value) => process.stdout.write(JSON.stringify(value));
const fail = (error_code, error = '/synthetic/private/path raw diagnostic must not escape') => {
  process.stderr.write(JSON.stringify({ ok: false, error_code, error }));
  process.exitCode = 1;
};
await appendFile(journal, JSON.stringify({ args }) + '\n');
if (args.join(' ') === '--format text workflow --help') {
  process.stdout.write([
    scenario === 'native17' ? 'Control version 17, native chat' : 'VibeMux CLI fixture',
    '  vibemuxctl workflow prepare --request <file> --policy <file> [--project-root <path>]',
    '  vibemuxctl workflow status|pause|cancel|purge <workflow_id> [--project-root <path>]',
    '  vibemuxctl tasks list [--after <task_id>] [--limit <limit>] [--project-root <path>]',
    '  vibemuxctl tasks show <task_id> [--project-root <path>]',
    ...(['main', 'native17'].includes(scenario) ? [] : [
      '  vibemuxctl workflow intake --request-file <path> [--project-root <path>]',
      '  vibemuxctl workflow followup --request-file <path> [--project-root <path>]',
      '  vibemuxctl workflow reports <workflow_id> [--after-sequence <n>] [--include-content] [--project-root <path>]',
    ]),
  ].join('\n'));
} else if (args[0] !== '--format' || args[1] !== 'json'
  || args.at(-2) !== '--project-root' || !path.isAbsolute(args.at(-1))) {
  fail('cli_invalid_arguments');
} else {
  const [group, verb] = args.slice(2);
  let document;
  if (['intake', 'followup'].includes(verb)) {
    const filename = args[args.indexOf('--request-file') + 1];
    document = JSON.parse(await readFile(filename, 'utf8'));
    const mode = (await stat(filename)).mode & 0o777;
    await appendFile(journal, JSON.stringify({ document, filename, mode }) + '\n');
  }
  if (scenario === 'output_limit') {
    process.stdout.write('x'.repeat(4096));
  } else if (scenario === 'invalid_utf8') {
    process.stdout.write(Buffer.from([0xff]));
  } else if (scenario === 'timeout') {
    await new Promise((resolve) => setTimeout(resolve, 10000));
  } else if (scenario === 'unknown' && document) {
    fail('control_timeout');
  } else if (scenario === 'rejected' && document) {
    fail('workflow_contract_mismatch');
  } else if (scenario === 'malformed_receipt' && document) {
    print({ ok: true, intake: { event_id: document.event_id, schema_version: 1 } });
  } else if (group === 'tasks' && verb === 'list') {
    print(taskPage);
  } else if (group === 'tasks' && verb === 'show') {
    print(taskDetail);
  } else if (group === 'workflow' && verb === 'status') {
    print({ ok: true, workflow });
  } else if (group === 'workflow' && verb === 'reports') {
    const after = Number(args[args.indexOf('--after-sequence') + 1]);
    // Include content even when not requested: the bridge must still strip it.
    print({ ok: true, worker_reports: {
      schema_version: 1, reports: reports.filter((entry) => entry.sequence > after), next: 4, more: false,
    } });
  } else if (group === 'workflow' && ['pause', 'cancel'].includes(verb)) {
    print({ ok: true, workflow: {
      workflow_id: WORKFLOW, phase: verb === 'pause' ? 'paused' : 'cancelled', signalled: false,
    } });
  } else if (document) {
    let previous;
    try { previous = JSON.parse(await readFile(receiptFile, 'utf8')); } catch { /* First synthetic call. */ }
    const duplicate = previous?.document.event_id === document.event_id;
    await writeFile(receiptFile, JSON.stringify({ document }));
    if (verb === 'intake') {
      print({ ok: true, intake: {
        schema_version: 1, event_id: document.event_id.toLowerCase(), authority: 'authenticated_operator',
        prepared: {
          workflow_id: WORKFLOW, phase: 'prepared', start_contract: CONTRACT,
          contracts: [], narrowing_count: 0, duplicate,
        },
        mandate: { original_request_sha256: CONTRACT, policy_digest: CONTRACT,
          base_commit: 'b'.repeat(40), contracts: [] },
      } });
    } else {
      print({ ok: true, followup: {
        duplicate, context_only: true, message: {
          schema_version: 1, state: 'admitted', recipient_sequence: 1, updated_at_ms: 0,
          envelope: {
            message_id: MESSAGE, workflow_id: WORKFLOW,
            sender: { kind: 'operator' }, recipient: {
              kind: 'worker', task_key: document.task_key, session_id: document.session_id.toLowerCase(),
            },
            contract_id: document.contract_id.toLowerCase(), contract_version: document.contract_version,
            kind: document.kind, body_sha256: createHash('sha256').update(document.text).digest('hex'),
          },
        },
      } });
    }
  } else {
    fail('cli_invalid_arguments');
  }
}
