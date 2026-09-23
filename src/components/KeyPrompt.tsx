import { useState } from "react";
import { api, errorMessage } from "../lib/api";
import { Help, Label, pillField, primaryButton } from "./fields";

/**
 * First-launch prompt for the Gemini API key (M2 item 10). The key goes straight to Windows
 * Credential Manager. The full Settings screen is BACKLOG B1.
 */
export function KeyPrompt({ onDone }: { onDone: () => void }) {
  const [key, setKey] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);

  async function save() {
    setBusy(true);
    setStatus("Checking the key with Gemini…");
    try {
      await api.setApiKey(key);
      const models = await api.testConnection();
      setStatus(`Connected: ${models.length} models available.`);
      onDone();
    } catch (e) {
      setStatus(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-ink/30 p-6">
      <div role="dialog" aria-modal="true" aria-labelledby="key-title" className="flex w-full max-w-lg flex-col gap-5 rounded-[32px] bg-card p-10 shadow-2xl">
        <h2 id="key-title" className="m-0 font-display text-2xl font-semibold">Connect Gemini</h2>
        <p className="m-0 text-muted">Personas and answers are written by Google Gemini. Paste an API key from Google AI Studio. It is stored in Windows Credential Manager, never in project files.</p>
        <div className="flex flex-col gap-2">
          <Label htmlFor="api-key">Gemini API key</Label>
          <input id="api-key" type="password" autoComplete="off" className={pillField} value={key} onChange={(e) => setKey(e.target.value)} />
          <Help>For confidential projects, use a key from a paid Gemini project.</Help>
        </div>
        <div className="flex items-center justify-between gap-4">
          <span role="status" className="text-sm text-muted">{status}</span>
          <button type="button" className={primaryButton} disabled={!key.trim() || busy} onClick={save}>Save and test</button>
        </div>
      </div>
    </div>
  );
}
