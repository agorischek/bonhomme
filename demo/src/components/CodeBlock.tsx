import hljs from 'highlight.js/lib/core'
import csharp from 'highlight.js/lib/languages/csharp'
import elixir from 'highlight.js/lib/languages/elixir'
import go from 'highlight.js/lib/languages/go'
import javascript from 'highlight.js/lib/languages/javascript'
import json from 'highlight.js/lib/languages/json'
import markdown from 'highlight.js/lib/languages/markdown'
import python from 'highlight.js/lib/languages/python'
import rust from 'highlight.js/lib/languages/rust'
import typescript from 'highlight.js/lib/languages/typescript'
import { useMemo } from 'react'
import './CodeBlock.css'

hljs.registerLanguage('csharp', csharp)
hljs.registerLanguage('elixir', elixir)
hljs.registerLanguage('go', go)
hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('json', json)
hljs.registerLanguage('markdown', markdown)
hljs.registerLanguage('python', python)
hljs.registerLanguage('rust', rust)
hljs.registerLanguage('typescript', typescript)

const extensionLanguages: Record<string, string> = {
  cs: 'csharp',
  ex: 'elixir',
  exs: 'elixir',
  go: 'go',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  jsonc: 'json',
  md: 'markdown',
  markdown: 'markdown',
  py: 'python',
  pyi: 'python',
  rs: 'rust',
  ts: 'typescript',
  tsx: 'typescript',
}

function escapeHtml(value: string) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}

function languageForPath(path?: string) {
  const extension = path?.split('.').pop()?.toLowerCase()
  return extension ? extensionLanguages[extension] : undefined
}

export function CodeBlock({
  code,
  path,
  className,
}: {
  code: string
  path?: string
  className: string
}) {
  const language = languageForPath(path)
  const html = useMemo(() => {
    if (!code) return ''
    if (language && hljs.getLanguage(language)) {
      return hljs.highlight(code, { language, ignoreIllegals: true }).value
    }
    return escapeHtml(code)
  }, [code, language])

  return (
    <pre className={`${className} syntax-code`} data-language={language ?? 'text'}>
      <code dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  )
}
