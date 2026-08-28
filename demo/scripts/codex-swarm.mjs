#!/usr/bin/env node
import { promises as fs } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { codexEnvironment } from './swarm-env.mjs'
import { apiConflictWorkspace, resolveApiConflictWorkspace } from './swarm-paths.mjs'
import { run } from './swarm-run.mjs'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const demoRoot = path.resolve(__dirname, '..')
const repoRoot = path.resolve(demoRoot, '..')
const bonhommeBin = path.join(
  repoRoot,
  'target',
  'debug',
  process.platform === 'win32' ? 'bonhomme.exe' : 'bonhomme',
)

const ADD_SCENARIOS = [
  ['subscriptionRenewalSummary', 'subscription renewal summary', 'subscriptions'],
  ['crossDockReadiness', 'cross-dock readiness', 'warehouse'],
  ['paymentExceptionNarrative', 'payment exception narrative', 'payments'],
  ['deliveryExceptionNote', 'delivery exception note', 'last-mile'],
  ['refundSlaNote', 'refund SLA note', 'refunds'],
  ['loyaltySaveOffer', 'loyalty save offer', 'customer care'],
  ['regionalCapacityForecast', 'regional capacity forecast', 'capacity'],
  ['handoffChecklist', 'handoff checklist', 'operations'],
  ['packingPriority', 'packing priority', 'packing'],
  ['backorderRecovery', 'backorder recovery', 'supply'],
  ['giftWrapSignal', 'gift wrap signal', 'extras'],
  ['priorityQueueNote', 'priority queue note', 'queueing'],
]

const REALISTIC_PLAN = [
  addMissionTemplate('subscriptionRenewalSummary', 'add subscription renewal summary', 'subscriptions'),
  updateMissionTemplate('summarizeCart', 'update cart summary wording', 'checkout', 'green', 12),
  renameMissionTemplate('quoteFulfillmentWindow', 'quoteDeliveryWindow', 'rename fulfillment quote'),
  deleteMissionTemplate('customerHoldReason', 'delete obsolete hold reason'),
  addMissionTemplate('crossDockReadiness', 'add cross-dock readiness', 'warehouse'),
  updateMissionTemplate('reserveInventory', 'update inventory reservation rule', 'warehouse', 'amber', 18),
  renameMissionTemplate('releaseInventoryHold', 'clearInventoryHold', 'rename inventory hold release'),
  deleteMissionTemplate('auditTrailLabel', 'delete legacy audit label'),
  addMissionTemplate('paymentExceptionNarrative', 'add payment exception narrative', 'payments'),
  updateMissionTemplate('scorePaymentRisk', 'update payment risk score', 'risk', 'amber', 24),
  {
    ...addMissionTemplate('duplicateRiskReview', 'add duplicate risk review', 'risk'),
    body:
      'const stages = this.listOrders().join(" -> ");\n' +
      'return `${this.displayName()} duplicate risk review on risk lane: amber within 24h (${stages})`;',
    conflict: true,
  },
  renameMissionTemplate('authorizePayment', 'authorizePaymentCapture', 'rename payment authorization'),
  deleteMissionTemplate('warehouseBatchKey', 'delete warehouse batch key'),
  addMissionTemplate('deliveryExceptionNote', 'add delivery exception note', 'last-mile'),
  updateMissionTemplate('chooseCarrier', 'update carrier routing logic', 'routing', 'blue', 30),
  renameMissionTemplate('schedulePickup', 'bookCarrierPickup', 'rename carrier pickup scheduling'),
  deleteMissionTemplate('supportEscalationPath', 'delete support escalation path'),
  addMissionTemplate('refundSlaNote', 'add refund SLA note', 'refunds'),
  updateMissionTemplate('calculateTaxRegion', 'update tax region copy', 'tax', 'silver', 18),
  renameMissionTemplate('issueRefundPreview', 'previewRefundCredit', 'rename refund preview'),
  addMissionTemplate('loyaltySaveOffer', 'add loyalty save offer', 'retention'),
  {
    ...addMissionTemplate('duplicateRiskReview', 'add duplicate risk review again', 'risk'),
    body:
      'const stages = this.listOrders().join(" -> ");\n' +
      'return `${this.displayName()} duplicate risk review on risk lane: red within 48h (${stages})`;',
    conflict: true,
  },
  updateMissionTemplate('loyaltyTierMessage', 'update loyalty tier message', 'loyalty', 'gold', 12),
  addMissionTemplate('regionalCapacityForecast', 'add regional capacity forecast', 'capacity'),
  addMissionTemplate('handoffChecklist', 'add operations handoff checklist', 'operations'),
  {
    ...updateMissionTemplate('summarizeCart', 'conflicting cart summary update', 'checkout', 'silver', 36),
    conflict: true,
  },
  {
    ...renameMissionTemplate('chooseCarrier', 'selectCarrierRoute', 'conflicting carrier rename'),
    conflict: true,
  },
  {
    ...deleteMissionTemplate('calculateTaxRegion', 'conflicting tax region delete'),
    conflict: true,
  },
  {
    ...deleteMissionTemplate('scorePaymentRisk', 'conflicting risk score delete'),
    conflict: true,
  },
  addMissionTemplate('packingPriority', 'add packing priority', 'packing'),
]

function parseArgs(argv) {
  const options = {
    agents: 30,
    concurrency: 30,
    includeConflicts: true,
    reset: true,
    merge: false,
    dryRun: false,
    keepWorkdir: false,
    model: process.env.CODEX_SWARM_MODEL ?? 'gpt-5.3-codex-spark',
    codex: process.env.CODEX_BIN ?? 'codex',
    apiBase: process.env.BONHOMME_API_BASE ?? null,
  }

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    const [key, inlineValue] = arg.split('=', 2)
    const nextValue = () => inlineValue ?? argv[++index]

    switch (key) {
      case '--agents':
        options.agents = Number(nextValue())
        break
      case '--concurrency':
        options.concurrency = Number(nextValue())
        break
      case '--model':
        options.model = nextValue()
        break
      case '--codex':
        options.codex = nextValue()
        break
      case '--api-base':
        options.apiBase = nextValue()
        break
      case '--include-conflicts':
        options.includeConflicts = true
        break
      case '--no-conflicts':
        options.includeConflicts = false
        break
      case '--no-reset':
        options.reset = false
        break
      case '--merge':
        options.merge = true
        break
      case '--dry-run':
        options.dryRun = true
        break
      case '--keep-workdir':
        options.keepWorkdir = true
        break
      case '--help':
      case '-h':
        printHelp()
        process.exit(0)
        break
      default:
        throw new Error(`Unknown option: ${arg}`)
    }
  }

  if (!Number.isInteger(options.agents) || options.agents < 1) {
    throw new Error('--agents must be a positive integer')
  }
  if (!Number.isInteger(options.concurrency) || options.concurrency < 1) {
    throw new Error('--concurrency must be a positive integer')
  }

  options.concurrency = Math.min(options.concurrency, options.agents)
  return options
}

function printHelp() {
  console.log(`
Run a real Codex swarm against the local bonhomme demo repository.

Usage:
  npm run swarm:codex -- [options]

Options:
  --agents <n>          Number of Codex workers to launch. Default: 30
  --concurrency <n>     Maximum workers running at once. Default: 30
  --model <name>        Codex model name. Default: CODEX_SWARM_MODEL or gpt-5.3-codex-spark
  --codex <path>        Codex binary. Default: CODEX_BIN or codex
  --api-base <url>      Submit slices through a running bonhomme API instead of the CLI
  --include-conflicts   Intentionally create duplicate semantic work. Default
  --no-conflicts        Replace conflict slots with independent work
  --no-reset            Reuse the existing bonhomme-demo repository
  --merge               Merge each worker as soon as its slice is submitted; resolve conflicts
  --dry-run             Use deterministic template edits instead of launching Codex
  --keep-workdir        Keep .bonhomme/swarm/<run-id> after the run
`)
}

function branchName(number) {
  return `agent-${String(number).padStart(3, '0')}`
}

function addMissionTemplate(methodName, label, lane) {
  return {
    kind: 'add',
    methodName,
    label,
    lane,
    signal: 'green',
    promiseHours: 24,
    conflict: false,
  }
}

function updateMissionTemplate(targetName, label, lane, signal, promiseHours) {
  return {
    kind: 'update',
    targetName,
    label,
    lane,
    signal,
    promiseHours,
    conflict: false,
  }
}

function renameMissionTemplate(targetName, newName, label) {
  return {
    kind: 'rename',
    targetName,
    newName,
    label,
    conflict: false,
  }
}

function deleteMissionTemplate(targetName, label) {
  return {
    kind: 'delete',
    targetName,
    label,
    conflict: false,
  }
}

function missionFor(number, includeConflicts) {
  const agent = branchName(number)
  const planned = REALISTIC_PLAN[number - 1]
  if (planned && (includeConflicts || !planned.conflict)) {
    return { ...planned, agent }
  }

  const [baseName, label, lane] = ADD_SCENARIOS[(number - 1) % ADD_SCENARIOS.length]
  const cycle = Math.floor((number - 1) / ADD_SCENARIOS.length) + 1
  const signal = ['green', 'amber', 'blue', 'silver', 'gold'][number % 5]
  return {
    kind: 'add',
    agent,
    methodName: cycle === 1 ? baseName : `${baseName}${cycle}`,
    label,
    lane,
    signal,
    promiseHours: 12 + (number % 6) * 6,
    conflict: false,
  }
}

function missionMethodName(mission) {
  if (mission.kind === 'rename') return mission.newName
  return mission.methodName ?? mission.targetName
}

function agentNumber(mission) {
  return mission.agent.match(/\d+/)?.[0] ?? '000'
}

function methodBody(mission) {
  if (mission.body) return mission.body
  const lane = mission.lane ?? 'operations'
  const signal = mission.signal ?? 'green'
  const promiseHours = mission.promiseHours ?? 24
  return [
    'const stages = this.listOrders().join(" -> ");',
    `return \`\${this.displayName()} ${mission.label} on ${lane} lane: ${signal} within ${promiseHours}h (\${stages})\`;`,
  ].join('\n')
}

function methodSource(mission, name = missionMethodName(mission)) {
  return [
    `  ${name}(): string {`,
    ...methodBody(mission)
      .split('\n')
      .map((line) => `    ${line}`),
    '  }',
  ].join('\n')
}

function insertMethod(source, mission) {
  const name = missionMethodName(mission)
  if (source.includes(`${name}(): string`)) {
    return source
  }
  const marker = '\n}'
  const index = source.lastIndexOf(marker)
  if (index === -1) {
    throw new Error(`Could not find class closing brace for ${mission.agent}`)
  }
  return `${source.slice(0, index)}\n${methodSource(mission)}${source.slice(index)}`
}

function findMethodRange(source, methodName) {
  const signature = `  ${methodName}(): string {`
  const start = source.indexOf(signature)
  if (start === -1) {
    throw new Error(`Could not find method ${methodName}()`)
  }
  const bodyStart = source.indexOf('{', start)
  let depth = 0
  for (let index = bodyStart; index < source.length; index += 1) {
    const char = source[index]
    if (char === '{') depth += 1
    if (char === '}') depth -= 1
    if (depth === 0) {
      return { start, end: index + 1 }
    }
  }
  throw new Error(`Could not find closing brace for ${methodName}()`)
}

function replaceMethod(source, targetName, replacement) {
  const range = findMethodRange(source, targetName)
  return `${source.slice(0, range.start)}${replacement}${source.slice(range.end)}`
}

function renameMethod(source, targetName, newName) {
  const range = findMethodRange(source, targetName)
  const original = source.slice(range.start, range.end)
  return `${source.slice(0, range.start)}${original.replace(
    `${targetName}(): string`,
    `${newName}(): string`,
  )}${source.slice(range.end)}`
}

function deleteMethod(source, targetName) {
  const range = findMethodRange(source, targetName)
  const next = source[range.end] === '\n' ? range.end + 1 : range.end
  return `${source.slice(0, range.start)}${source.slice(next)}`.replace(/\n{3,}/g, '\n\n')
}

function applyTemplateEdit(source, mission) {
  if (mission.kind === 'add') return insertMethod(source, mission)
  if (mission.kind === 'update') {
    return replaceMethod(source, mission.targetName, methodSource(mission, mission.targetName))
  }
  if (mission.kind === 'rename') return renameMethod(source, mission.targetName, mission.newName)
  if (mission.kind === 'delete') return deleteMethod(source, mission.targetName)
  throw new Error(`Unsupported mission kind: ${mission.kind}`)
}

function conflictSummary(mergeResult) {
  return (mergeResult.conflicts ?? [])
    .map((conflict) => `${conflict.reason}: ${conflict.detail}`)
    .join('\n')
}

function promptFor(mission) {
  const shared = `You are ${mission.agent}, one worker in a 30-agent bonhomme source-control demo.

Edit only OrderService.ts in this directory. Do not edit any other file. Do not run package managers.
`

  if (mission.kind === 'add') {
    return `
${shared}
Add exactly one method to the OrderService class. Do not change existing methods, imports, or formatting outside the new method.

Add this method exactly, using normal TypeScript class method syntax:

${methodSource(mission)}

When OrderService.ts has that method, stop.
`
  }

  if (mission.kind === 'update') {
    return `
${shared}
Update exactly one existing method: ${mission.targetName}().
Do not rename, add, or delete any method.

Replace the whole ${mission.targetName}() method with this exact method:

${methodSource(mission, mission.targetName)}

When OrderService.ts has that one method update, stop.
`
  }

  if (mission.kind === 'rename') {
    return `
${shared}
Rename exactly one existing method in the OrderService class:

  ${mission.targetName}() -> ${mission.newName}()

Only change the method name in its declaration. Keep its body exactly as it is. Do not add or delete methods.

When OrderService.ts has that one rename, stop.
`
  }

  if (mission.kind === 'delete') {
    return `
${shared}
Delete exactly one existing method from the OrderService class:

  ${mission.targetName}()

Do not change any other method, property, import, or formatting.

When OrderService.ts no longer has ${mission.targetName}(), stop.
`
  }

  return `
${shared}
Make the requested OrderService change, then stop.
`
}

function resolutionPromptFor(mission, mergeResult, attempt) {
  const duplicateName = mission.kind === 'add' ? missionMethodName(mission) : null
  const uniqueName = duplicateName
    ? `${duplicateName}ForAgent${agentNumber(mission)}${attempt > 1 ? `Retry${attempt}` : ''}`
    : null
  const duplicateResolutionRule = uniqueName
    ? `- If the conflict is only that ${duplicateName}() already exists, keep both intents ` +
      `by adding a clearly named method such as ${uniqueName}().`
    : ''

  return `
You are ${mission.agent}, one worker in a bonhomme semantic merge demo.

Your previous semantic merge conflicted with work that already landed:

${conflictSummary(mergeResult) || 'Bonhomme reported a semantic conflict.'}

This is a real Bonhomme conflict checkout. The workspace contains:

- CONFLICT.md: structured conflict details
- base/: the common base projection
- current/: the current target branch projection
- proposed/: your conflicted source projection
- resolved/: the only projection you should edit

Resolve the conflict by rebasing your original intent onto current/. Edit only files under resolved/.
Do not edit base/, current/, proposed/, CONFLICT.md, or the Bonhomme manifest. Do not run package managers.

Original mission:

${promptFor(mission).trim()}

Resolution rules:
- Preserve current/ behavior unless your original mission intentionally changes it.
- Preserve your proposed intent when it still makes sense.
- Do not add a generic "resolution" method just to bypass the conflict.
${duplicateResolutionRule}
- When resolved/ contains the final intended TypeScript projection, stop.
`
}

async function runJson(command, args, options) {
  const stdout = await run(command, args, { cwd: repoRoot, ...options })
  return JSON.parse(stdout)
}

async function apiFetch(options, apiPath, body) {
  const url = new URL(apiPath, options.apiBase)
  const headers = {}
  if (body !== undefined) headers['content-type'] = 'application/json'
  if (process.env.BONHOMME_API_TOKEN) {
    headers.authorization = `Bearer ${process.env.BONHOMME_API_TOKEN}`
  }
  if (process.env.BONHOMME_API_TENANTS) {
    headers['x-bonhomme-tenant'] = process.env.BONHOMME_API_TENANTS
  }
  const response = await fetch(url, {
    method: body === undefined ? 'GET' : 'POST',
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!response.ok) {
    const errorBody = await response.json().catch(() => ({ error: response.statusText }))
    throw new Error(`${url.pathname}: ${errorBody.error ?? response.statusText}`)
  }
  return response.json()
}

async function prepareBonhomme(options) {
  if (!options.apiBase) {
    console.log('Building bonhomme CLI...')
    await run('cargo', ['build', '-p', 'bonhomme'], { cwd: repoRoot })
  }

  if (options.reset) {
    console.log('Resetting bonhomme-demo...')
    if (options.apiBase) {
      await apiFetch(options, '/api/demo/reset', {})
    } else {
      await run(bonhommeBin, ['demo', 'reset'], { cwd: repoRoot })
    }
  }
}

async function createSlice(options, runDir, mission) {
  const agentDir = path.join(runDir, mission.agent)
  await fs.mkdir(agentDir, { recursive: true })
  const slice = options.apiBase
    ? (
        await apiFetch(options, '/api/demo/agent-slice', {
          branchName: mission.agent,
          symbol: 'OrderService',
        })
      ).slice
    : await createCliSlice(mission)
  if (!slice.files?.[0]) {
    throw new Error(`No files rendered for ${mission.agent}`)
  }

  const sourcePath = path.join(agentDir, 'OrderService.ts')
  const slicePath = path.join(agentDir, 'slice.json')
  const promptPath = path.join(agentDir, 'PROMPT.md')
  await fs.writeFile(sourcePath, slice.files[0].content)
  await fs.writeFile(slicePath, `${JSON.stringify(slice, null, 2)}\n`)
  await fs.writeFile(promptPath, promptFor(mission))
  return { agentDir, sourcePath, slicePath, promptPath }
}

async function createCliSlice(mission) {
  await run(
    bonhommeBin,
    [
      'branch',
      'create',
      '--repo',
      'bonhomme-demo',
      '--name',
      mission.agent,
      '--base',
      'main',
    ],
    { cwd: repoRoot },
  )
  return runJson(bonhommeBin, [
    'slice',
    'create',
    '--repo',
    'bonhomme-demo',
    '--branch',
    mission.agent,
    '--symbol',
    'OrderService',
    '--json',
  ])
}

async function runCodex(options, mission, paths, prompt = promptFor(mission)) {
  if (options.dryRun) {
    const source = await fs.readFile(paths.sourcePath, 'utf8')
    await fs.writeFile(paths.sourcePath, applyTemplateEdit(source, mission))
    return
  }

  const logPath = path.join(paths.agentDir, 'codex.log')
  const finalPath = path.join(paths.agentDir, 'codex-final.md')
  await run(
    options.codex,
    [
      'exec',
      '--skip-git-repo-check',
      '--ephemeral',
      '--sandbox',
      'workspace-write',
      '--cd',
      paths.agentDir,
      '--model',
      options.model,
      '--output-last-message',
      finalPath,
      prompt,
    ],
    { cwd: paths.agentDir, logPath, env: codexEnvironment(), replaceEnv: true },
  )
}

async function applySlice(options, mission, paths) {
  const slice = JSON.parse(await fs.readFile(paths.slicePath, 'utf8'))
  slice.files[0].content = await fs.readFile(paths.sourcePath, 'utf8')
  await fs.writeFile(paths.slicePath, `${JSON.stringify(slice, null, 2)}\n`)
  if (options.apiBase) {
    await apiFetch(options, '/api/demo/apply-slice', {
      agent: mission.agent,
      title: `${mission.agent}: ${mission.label}`,
      slice,
    })
  } else {
    await run(
      bonhommeBin,
      [
        'slice',
        'apply',
        '--repo',
        'bonhomme-demo',
        '--modified',
        paths.slicePath,
        '--title',
        `${mission.agent}: ${mission.label}`,
        '--agent',
        mission.agent,
      ],
      { cwd: repoRoot },
    )
  }
}

async function mergeBranch(options, branchName) {
  return options.apiBase
    ? apiFetch(options, `/api/demo/merge/${branchName}`, {})
    : runJson(bonhommeBin, [
        'merge',
        '--repo',
        'bonhomme-demo',
        '--source',
        branchName,
        '--target',
        'main',
      ])
}

function conflictIdFrom(result, mission) {
  if (result.conflictId) return result.conflictId
  throw new Error(`${mission.agent}: merge conflicted but did not return conflictId`)
}

function conflictDirectory(conflictId) {
  return path.join(repoRoot, '.bonhomme', 'conflicts', conflictId)
}

async function pathExists(target) {
  try {
    await fs.access(target)
    return true
  } catch {
    return false
  }
}

async function checkoutConflict(options, conflictId) {
  if (options.apiBase) {
    const checkout = await apiFetch(options, `/api/conflicts/${conflictId}/checkout`, {})
    return resolveApiConflictWorkspace(repoRoot, conflictId, checkout.directory)
  }

  const directory = conflictDirectory(conflictId)
  if (await pathExists(path.join(directory, '.bonhomme-conflict.json'))) {
    return directory
  }
  await run(bonhommeBin, ['conflict', 'checkout', conflictId], { cwd: repoRoot })
  return directory
}

async function findFileByName(directory, fileName) {
  const entries = await fs.readdir(directory, { withFileTypes: true })
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name)
    if (entry.isDirectory()) {
      const nested = await findFileByName(entryPath, fileName)
      if (nested) return nested
    } else if (entry.isFile() && entry.name === fileName) {
      return entryPath
    }
  }
  return null
}

async function resolvedOrderServicePath(conflictDir) {
  const resolvedDir = path.join(conflictDir, 'resolved')
  const sourcePath = await findFileByName(resolvedDir, 'OrderService.ts')
  if (!sourcePath) {
    throw new Error(`No resolved OrderService.ts found in ${resolvedDir}`)
  }
  return sourcePath
}

async function resetResolvedProjection(conflictDir) {
  const currentDir = path.join(conflictDir, 'current')
  const resolvedDir = path.join(conflictDir, 'resolved')
  await fs.rm(resolvedDir, { recursive: true, force: true })
  await fs.cp(currentDir, resolvedDir, { recursive: true })
}

function resolutionMissionFor(source, mission, attempt) {
  if (mission.kind === 'add' && source.includes(`${mission.methodName}(): string`)) {
    return {
      ...mission,
      methodName: `${mission.methodName}ForAgent${agentNumber(mission)}${
        attempt > 1 ? `Retry${attempt}` : ''
      }`,
      label: `${mission.label} for ${mission.agent}`,
    }
  }
  return mission
}

function applyResolutionTemplate(source, mission, attempt) {
  const resolutionMission = resolutionMissionFor(source, mission, attempt)
  try {
    return applyTemplateEdit(source, resolutionMission)
  } catch {
    return source
  }
}

async function runConflictResolver(options, mission, conflictDir, mergeResult, attempt) {
  const prompt = resolutionPromptFor(mission, mergeResult, attempt)
  await fs.writeFile(path.join(conflictDir, `RESOLUTION-${mission.agent}.md`), prompt)

  if (options.dryRun) {
    const sourcePath = await resolvedOrderServicePath(conflictDir)
    const source = await fs.readFile(sourcePath, 'utf8')
    await fs.writeFile(sourcePath, applyResolutionTemplate(source, mission, attempt))
    return
  }

  const logPath = path.join(conflictDir, `${mission.agent}-resolve.log`)
  const finalPath = path.join(conflictDir, `${mission.agent}-resolve-final.md`)
  await run(
    options.codex,
    [
      'exec',
      '--skip-git-repo-check',
      '--ephemeral',
      '--sandbox',
      'workspace-write',
      '--cd',
      conflictDir,
      '--model',
      options.model,
      '--output-last-message',
      finalPath,
      prompt,
    ],
    { cwd: conflictDir, logPath, env: codexEnvironment(), replaceEnv: true },
  )
}

function resolutionPayload(result) {
  if (result.resolved) return { status: 'resolved', body: result.resolved }
  if (result.stillConflicting) return { status: 'stillConflicting', body: result.stillConflicting }
  throw new Error(`Unknown resolve result shape: ${JSON.stringify(result)}`)
}

async function resolveConflict(options, mission, conflictId, conflictDir, acceptDrop = false) {
  const body = {
    dir: options.apiBase ? apiConflictWorkspace(repoRoot, conflictDir) : conflictDir,
    agent: mission.agent,
    acceptDrop,
  }
  return options.apiBase
    ? apiFetch(options, `/api/conflicts/${conflictId}/resolve`, body)
    : runJson(bonhommeBin, [
        'conflict',
        'resolve',
        conflictId,
        '--dir',
        conflictDir,
        '--agent',
        mission.agent,
        ...(acceptDrop ? ['--accept-drop'] : []),
      ])
}

async function mergeOrResolve(options, mission) {
  const result = await mergeBranch(options, mission.agent)
  if (result.outcome === 'SAFE_MERGE') {
    console.log(`${mission.agent}: merged ${result.appendedOperations.length} ops`)
    return result
  }

  console.log(`${mission.agent}: semantic conflict (${result.conflicts[0]?.detail ?? 'review required'})`)
  const conflictId = conflictIdFrom(result, mission)
  const conflictDir = await checkoutConflict(options, conflictId)
  let lastDiagnostic = conflictSummary(result) || 'semantic conflict'

  for (let attempt = 1; attempt <= 2; attempt += 1) {
    await resetResolvedProjection(conflictDir)
    console.log(`${mission.agent}: resolving conflict ${conflictId} in ${path.relative(repoRoot, conflictDir)}`)
    await runConflictResolver(options, mission, conflictDir, result, attempt)
    const resolveResult = await resolveConflict(options, mission, conflictId, conflictDir)
    const resolved = resolutionPayload(resolveResult)
    if (resolved.status === 'resolved') {
      const appendedOperations = resolved.body.appendedOperations ?? resolved.body.appended_operations ?? []
      const appended = appendedOperations.length
      console.log(`${mission.agent}: resolved conflict ${conflictId} with ${appended} ops`)
      return resolveResult
    }
    lastDiagnostic = resolved.body.diagnostic ?? 'resolution still conflicted'
    console.log(`${mission.agent}: resolution attempt ${attempt} still conflicted (${lastDiagnostic})`)
  }

  throw new Error(`${mission.agent}: conflict resolution attempts exhausted: ${lastDiagnostic}`)
}

async function runWorker(options, mission, paths, stateLock) {
  console.log(`${mission.agent}: ${options.dryRun ? 'writing template edit' : `launching ${options.model}`}`)
  await runCodex(options, mission, paths)
  await stateLock(async () => {
    console.log(`${mission.agent}: applying semantic slice`)
    await applySlice(options, mission, paths)
    if (options.merge) {
      await mergeOrResolve(options, mission)
    } else {
      console.log(`${mission.agent}: awaiting merge`)
    }
  })
  return mission.agent
}

function serialQueue() {
  let current = Promise.resolve()
  return async (task) => {
    const runTask = current.then(task, task)
    current = runTask.catch(() => {})
    return runTask
  }
}

async function runPool(items, limit, worker) {
  const results = []
  let next = 0
  const runners = Array.from({ length: limit }, async () => {
    while (next < items.length) {
      const index = next
      const item = items[index]
      next += 1
      try {
        results[index] = { ok: true, item, value: await worker(item) }
      } catch (error) {
        const agent = item?.mission?.agent ?? `worker-${index + 1}`
        const message = error instanceof Error ? error.message : String(error)
        console.error(`${agent}: failed (${message.split('\n')[0]})`)
        results[index] = { ok: false, item, error: message }
      }
    }
  })
  await Promise.all(runners)
  return results
}

async function mergeBranches(options, missions) {
  let safe = 0
  let conflicts = 0
  for (const mission of missions) {
    const result = await mergeBranch(options, mission.agent)
    if (result.outcome === 'SAFE_MERGE') {
      safe += 1
      console.log(`${mission.agent}: merged ${result.appendedOperations.length} ops`)
    } else {
      conflicts += 1
      console.log(`${mission.agent}: semantic conflict (${result.conflicts[0]?.detail ?? 'review required'})`)
    }
  }
  console.log(`Merge summary: ${safe} safe, ${conflicts} conflicts`)
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const runId = new Date().toISOString().replaceAll(':', '').replaceAll('.', '')
  const runDir = path.join(repoRoot, '.bonhomme', 'swarm', runId)
  const missions = Array.from({ length: options.agents }, (_, index) =>
    missionFor(index + 1, options.includeConflicts),
  )

  await fs.mkdir(runDir, { recursive: true, mode: 0o700 })
  await fs.chmod(runDir, 0o700)
  await prepareBonhomme(options)
  console.log(
    `Launching ${options.agents} ${options.dryRun ? 'template' : options.model} workers with concurrency ${options.concurrency}`,
  )
  if (options.apiBase) {
    console.log(`Submitting semantic operations through ${options.apiBase}`)
  }
  console.log(`Swarm workspace: ${path.relative(repoRoot, runDir)}`)

  const prepared = []
  for (const mission of missions) {
    console.log(`${mission.agent}: creating branch and slice`)
    prepared.push({ mission, paths: await createSlice(options, runDir, mission) })
  }

  const stateLock = serialQueue()
  const workerResults = await runPool(prepared, options.concurrency, ({ mission, paths }) =>
    runWorker(options, mission, paths, stateLock),
  )
  const failedResults = workerResults.filter((result) => !result?.ok)

  if (!options.keepWorkdir && !options.dryRun) {
    console.log(`Worker logs kept in ${path.relative(repoRoot, runDir)}`)
  } else if (!options.keepWorkdir && options.dryRun) {
    await fs.rm(runDir, { recursive: true, force: true })
  }

  if (failedResults.length > 0) {
    console.error(
      `Swarm finished with ${failedResults.length} failed worker(s). Inspect ${path.relative(repoRoot, runDir)} for logs.`,
    )
    process.exitCode = 1
    return
  }

  console.log(
    options.merge
      ? 'Swarm complete. Workers merged as they finished; semantic conflicts were resolved through Bonhomme.'
      : 'Swarm complete. Open the dashboard and run the merge queue when you are ready.',
  )
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
