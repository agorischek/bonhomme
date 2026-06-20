import hljs from 'highlight.js/lib/core'
import go from 'highlight.js/lib/languages/go'
import javascript from 'highlight.js/lib/languages/javascript'
import json from 'highlight.js/lib/languages/json'
import markdown from 'highlight.js/lib/languages/markdown'
import rust from 'highlight.js/lib/languages/rust'
import typescript from 'highlight.js/lib/languages/typescript'
import { useMemo } from 'react'
import './CodeBlock.css'

hljs.registerLanguage('go', go)
hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('json', json)
hljs.registerLanguage('markdown', markdown)
hljs.registerLanguage('rust', rust)
hljs.registerLanguage('typescript', typescript)

const extensionLanguages: Record<string, string> = {
  go: 'go',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  jsonc: 'json',
  md: 'markdown',
  markdown: 'markdown',
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
