import { spawn } from 'node:child_process'
import { createWriteStream, promises as fs } from 'node:fs'
import { PassThrough, Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { StringDecoder } from 'node:string_decoder'

import {
  boundedDiagnosticTail,
  createStreamingRedactor,
  redactSubprocessOutput,
} from './swarm-env.mjs'

export async function run(command, args, options = {}) {
  const environment = options.replaceEnv
    ? (options.env ?? {})
    : { ...process.env, ...(options.env ?? {}) }
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: environment,
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let stdout = ''
  let stderr = ''
  let diagnosticTail = ''
  const exit = new Promise((resolve, reject) => {
    child.once('error', reject)
    child.once('close', resolve)
  })

  let capture = Promise.resolve()
  if (options.logPath) {
    capture = streamSubprocessLog(child, options.logPath, environment, (chunk) => {
      diagnosticTail = boundedDiagnosticTail(diagnosticTail, chunk)
    })
  } else {
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
  }

  let code
  try {
    const results = await Promise.all([exit, capture])
    code = results[0]
  } catch (error) {
    child.kill()
    throw error
  }
  if (code !== 0) {
    const details = options.logPath
      ? diagnosticTail || `${command} exited with ${code}`
      : redactSubprocessOutput(stderr || stdout || `${command} exited with ${code}`, environment)
    throw new Error(details.trim())
  }
  return stdout
}

async function streamSubprocessLog(child, logPath, source, onOutput) {
  const combined = new PassThrough()
  let openStreams = 2
  const endCombined = () => {
    openStreams -= 1
    if (openStreams === 0) combined.end()
  }
  const abortCombined = (error) => combined.destroy(error)
  child.stdout.once('end', endCombined)
  child.stderr.once('end', endCombined)
  child.stdout.once('error', abortCombined)
  child.stderr.once('error', abortCombined)
  child.stdout.pipe(combined, { end: false })
  child.stderr.pipe(combined, { end: false })

  const decoder = new StringDecoder('utf8')
  const redactor = createStreamingRedactor(source)
  const redact = new Transform({
    transform(chunk, _encoding, callback) {
      try {
        const output = redactor.push(decoder.write(chunk))
        onOutput(output)
        callback(null, output)
      } catch (error) {
        callback(error)
      }
    },
    flush(callback) {
      try {
        const output = redactor.push(decoder.end()) + redactor.finish()
        onOutput(output)
        callback(null, output)
      } catch (error) {
        callback(error)
      }
    },
  })
  const log = createWriteStream(logPath, { flags: 'w', mode: 0o600 })
  await pipeline(combined, redact, log)
  await fs.chmod(logPath, 0o600)
}
