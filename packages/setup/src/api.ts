export async function validateConnection(url: string, key: string): Promise<{ name: string }> {
  const res = await fetch(`${url}/api/auth/me`, {
    headers: { Authorization: `Bearer ${key}` },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`Connection failed (${res.status}): ${text || res.statusText}`)
  }
  return res.json() as Promise<{ name: string }>
}

export async function sendHeartbeat(url: string, key: string): Promise<void> {
  const res = await fetch(`${url}/api/agents/heartbeat`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${key}`,
      'Content-Type': 'application/json',
    },
  })
  if (!res.ok) {
    throw new Error(`Heartbeat failed (${res.status})`)
  }
}
