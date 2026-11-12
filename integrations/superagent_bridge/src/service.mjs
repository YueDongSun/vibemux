import { createHash } from 'node:crypto';
import { BridgeError, assert, exactKeys, object, safeInteger, uuid } from './errors.mjs';
import { MAX_REQUEST_BYTES } from './config.mjs';
import { Capabilities } from './capabilities.mjs';
import { taskPage, taskDetail, workflowView, reportPage, attentionView } from './projection.mjs';

const ID = /^[a-zA-Z][a-zA-Z0-9_.-]{0,127}$/;

function byteLength(value) { return Buffer.byteLength(JSON.stringify(value)); }

function query(url, allowed) {
  for (const key of url.searchParams.keys()) {
    assert(allowed.includes(key) && url.searchParams.getAll(key).length === 1);
  }
}

function numberQuery(url, key, fallback, minimum = 0, maximum = Number.MAX_SAFE_INTEGER) {
  const text = url.searchParams.get(key);
  if (text === null) return fallback;
  assert(/^(0|[1-9][0-9]*)$/.test(text));
  const value = Number(text);
  assert(safeInteger(value, minimum, maximum));
  return value;
}

function contentQuery(url) {
  const value = url.searchParams.get('include_content');
  assert(value === null || value === 'true' || value === 'false');
  return value === 'true';
}

function checkedWorkflowId(value) {
  assert(uuid(value));
  return value.toLowerCase();
}

function validateFollowup(body, workflowId) {
  const keys = ['schema_version', 'event_id', 'workflow_id', 'expected_workflow_version',
    'task_key', 'session_id', 'contract_id', 'contract_version', 'kind', 'text'];
  exactKeys(body, keys, keys);
  assert(body.schema_version === 1 && uuid(body.event_id) && uuid(body.workflow_id)
    && body.workflow_id.toLowerCase() === workflowId && uuid(body.session_id));
  assert(safeInteger(body.expected_workflow_version, 1)
    && safeInteger(body.contract_version, 1, 0xffffffff));
  assert(typeof body.task_key === 'string' && ID.test(body.task_key));
  assert(typeof body.contract_id === 'string' && /^[a-f0-9]{64}$/i.test(body.contract_id));
  assert(['question', 'progress', 'finding'].includes(body.kind));
  assert(typeof body.text === 'string' && body.text.trim().length > 0
    && !body.text.includes('\0') && Buffer.byteLength(body.text) <= 4096);
  return body;
}

export class BridgeService {
  constructor(config, transport) {
    this.config = config;
    this.transport = transport;
    this.capabilities = new Capabilities(transport);
  }

  writeAllowed() {
    if (!this.config.enableWrites) throw new BridgeError('writes_disabled', 403);
  }

  project(id) {
    const project = this.config.projects.find((entry) => entry.id === id);
    if (!project) throw new BridgeError('unknown_project', 404);
    return project;
  }

  intakeDocument(project, body) {
    assert(object(body) && uuid(body.event_id));
    let request;
    let policy;
    if (Object.hasOwn(body, 'goal')) {
      exactKeys(body, ['event_id', 'goal', 'preset'], ['event_id', 'goal']);
      assert(typeof body.goal === 'string' && body.goal.trim().length > 0
        && !body.goal.includes('\0') && Buffer.byteLength(body.goal) <= 48 * 1024);
      const selected = body.preset === undefined && project.presets.length === 1
        ? project.presets[0] : project.presets.find((preset) => preset.id === body.preset);
      if (!selected) throw new BridgeError('preset_unavailable', 422);
      request = { ...structuredClone(selected.request), request_text: body.goal };
      policy = structuredClone(selected.policy);
    } else {
      exactKeys(body, ['event_id', 'request', 'policy'], ['event_id', 'request', 'policy']);
      if (!project.allowImport) throw new BridgeError('intake_import_disabled', 403);
      assert(object(body.request) && object(body.policy));
      request = structuredClone(body.request);
      policy = structuredClone(body.policy);
    }
    assert(typeof request.request_text === 'string' && request.request_text.trim().length > 0);
    assert(byteLength(request) <= 48 * 1024 && byteLength(policy) <= 8 * 1024,
      'body_too_large', 413);
    const envelope = { schema_version: 1, event_id: body.event_id, request, policy };
    assert(byteLength(envelope) <= MAX_REQUEST_BYTES, 'body_too_large', 413);
    return envelope;
  }

  async route(method, url, body) {
    const pieces = url.pathname.split('/').filter(Boolean).map((part) => {
      try { return decodeURIComponent(part); } catch { throw new BridgeError('invalid_request'); }
    });
    assert(url.pathname.startsWith('/api/v1/') && !url.pathname.includes('//'), 'route_not_found', 404);
    const [api, version, projects, projectId, resource, id, action, ...tail] = pieces;
    if (api !== 'api' || version !== 'v1' || projects !== 'projects' || tail.length) {
      throw new BridgeError('route_not_found', 404);
    }
    if (!projectId && method === 'GET') {
      query(url, []);
      return { ok: true, projects: this.config.projects.map((project) => ({
        id: project.id, label: project.label,
      })) };
    }
    const project = this.project(projectId);
    if (method === 'GET' && resource === 'capabilities' && !id) {
      query(url, []);
      return this.capabilities.view(this.config, project);
    }
    if (method === 'GET' && resource === 'tasks' && !action) {
      if (!id) {
        query(url, ['after', 'limit']);
        const limit = numberQuery(url, 'limit', 20, 1, 32);
        const after = url.searchParams.get('after');
        assert(after === null || uuid(after));
        await this.capabilities.require('tasks.list');
        const args = ['tasks', 'list', '--limit', String(limit)];
        if (after) args.push('--after', after);
        return taskPage(await this.transport.command(project, args), project.id);
      }
      query(url, []);
      const taskId = checkedWorkflowId(id);
      await this.capabilities.require('tasks.show');
      return taskDetail(await this.transport.command(project, ['tasks', 'show', taskId]),
        project.id, taskId);
    }
    if (method === 'POST' && resource === 'intake' && !id) {
      query(url, []);
      this.writeAllowed();
      const document = this.intakeDocument(project, body);
      await this.capabilities.require('workflow.intake');
      const result = await this.transport.document(project, 'intake', document);
      const prepared = result.intake?.prepared;
      if (!object(result.intake) || !uuid(result.intake.event_id)
        || result.intake.event_id.toLowerCase() !== body.event_id.toLowerCase()
        || result.intake.schema_version !== 1 || !object(prepared)
        || !uuid(prepared.workflow_id) || typeof prepared.phase !== 'string'
        || typeof prepared.duplicate !== 'boolean') {
        throw new BridgeError('cli_invalid_output', 502, 'unknown');
      }
      return { ok: true, project_id: project.id, event_id: body.event_id, intake: result.intake };
    }
    if (resource === 'workflows' && id) {
      const workflowId = checkedWorkflowId(id);
      if (method === 'GET' && !action) {
        query(url, []);
        await this.capabilities.require('workflow.status');
        return workflowView(await this.transport.command(project, ['workflow', 'status', workflowId]),
          project.id, workflowId);
      }
      if (method === 'GET' && ['reports', 'attention'].includes(action)) {
        query(url, ['after_sequence', 'include_content']);
        const after = numberQuery(url, 'after_sequence', 0);
        const include = contentQuery(url);
        if (include && !project.allowReportContent) throw new BridgeError('report_content_disabled', 403);
        await this.capabilities.require('workflow.reports');
        const args = ['workflow', 'reports', workflowId, '--after-sequence', String(after)];
        if (include) args.push('--include-content');
        const result = reportPage(await this.transport.command(project, args),
          project.id, workflowId, include);
        return action === 'attention' ? attentionView(result) : result;
      }
      if (method === 'POST' && action === 'followup') {
        query(url, []);
        this.writeAllowed();
        const document = validateFollowup(body, workflowId);
        await this.capabilities.require('workflow.followup');
        const result = await this.transport.document(project, 'followup', document);
        const envelope = result.followup?.message?.envelope;
        if (!object(result.followup) || result.followup.context_only !== true
          || typeof result.followup.duplicate !== 'boolean' || !object(envelope)
          || envelope.workflow_id !== workflowId
          || envelope.contract_id !== body.contract_id.toLowerCase()
          || envelope.contract_version !== body.contract_version
          || envelope.kind !== body.kind || envelope.sender?.kind !== 'operator'
          || envelope.recipient?.kind !== 'worker'
          || envelope.recipient.task_key !== body.task_key
          || envelope.recipient.session_id !== body.session_id.toLowerCase()
          || envelope.body_sha256 !== createHash('sha256').update(body.text).digest('hex')) {
          throw new BridgeError('cli_invalid_output', 502, 'unknown');
        }
        return { ok: true, project_id: project.id, event_id: body.event_id, followup: result.followup };
      }
      if (method === 'POST' && ['pause', 'cancel'].includes(action)) {
        query(url, []);
        this.writeAllowed();
        exactKeys(body, []);
        await this.capabilities.require('workflow.' + action);
        const result = await this.transport.command(project, ['workflow', action, workflowId],
          { mutation: true });
        if (!object(result.workflow)) throw new BridgeError('cli_invalid_output', 502, 'unknown');
        return { ok: true, project_id: project.id, workflow: result.workflow };
      }
    }
    throw new BridgeError('route_not_found', 404);
  }
}
