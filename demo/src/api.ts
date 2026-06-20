export const API_BASE = import.meta.env.VITE_BONHOMME_API ?? 'http://127.0.0.1:3030'

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    headers: { 'content-type': 'application/json', ...(init?.headers ?? {}) },
    ...init,
  })
  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: response.statusText }))
    throw new Error(body.error ?? response.statusText)
  }
  return response.json()
}

export const wait = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms))
