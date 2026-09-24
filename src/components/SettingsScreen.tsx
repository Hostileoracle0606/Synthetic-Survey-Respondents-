import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/api";
import type { Settings } from "../types/gen/Settings";
import type { UsageTier } from "../types/gen/UsageTier";
import { Help, Label, pillButton, pillField, primaryButton } from "./fields";

const USAGE_TIERS: { value: UsageTier; label: string }[] = [
  { value: "free", label: "Free" },
  { value: "tier1", label: "Tier 1" },
  { value: "tier2", label: "Tier 2" },
  { value: "tier3", label: "Tier 3" },
];

const DEFAULT_SETTINGS: Settings = { flashModel: null, proModel: null, usageTier: "free", flashPrice: null, proPrice: null };

const toText = (n: number | null | undefined) => (n == null ? "" : String(n));
const toNumber = (s: string) => (s.trim() === "" ? null : Number(s));

/**
 * Settings screen (BACKLOG B1): the Gemini key (test/delete), model overrides, usage tier and
 * price table. The rate limiter reads the usage tier on the next launch; the cost estimate before
 * a run (Step 3) and the live cost (Step 4) read the Flash price. Opens on first launch when no key is stored (from App.tsx),
 * and any later time from the gear button in the header.
 */
export function SettingsScreen({ onClose }: { onClose: () => void }) {
  const [hasKey, setHasKey] = useState<boolean | null>(null);
  const [key, setKey] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [settings, setSettingsState] = useState<Settings>(DEFAULT_SETTINGS);
  const [keyStatus, setKeyStatus] = useState("");
  const [saveStatus, setSaveStatus] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.hasApiKey().then(setHasKey).catch(() => setHasKey(false));
    api.getSettings().then(setSettingsState).catch(() => {});
  }, []);

  async function refreshModels() {
    try {
      setModels(await api.testConnection());
    } catch {
      setModels([]);
    }
  }

  useEffect(() => {
    if (hasKey) void refreshModels();
  }, [hasKey]);

  async function saveKey() {
    setBusy(true);
    setKeyStatus("Checking the key with Gemini…");
    try {
      await api.setApiKey(key);
      setKey("");
      setHasKey(true);
      const list = await api.testConnection();
      setModels(list);
      setKeyStatus(`Connected: ${list.length} models available.`);
    } catch (e) {
      setKeyStatus(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function testKey() {
    setBusy(true);
    setKeyStatus("Testing…");
    try {
      const list = await api.testConnection();
      setModels(list);
      setKeyStatus(`Connected: ${list.length} models available.`);
    } catch (e) {
      setKeyStatus(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function deleteKey() {
    setBusy(true);
    try {
      await api.deleteApiKey();
      setHasKey(false);
      setModels([]);
      setKeyStatus("Key deleted.");
    } catch (e) {
      setKeyStatus(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    setBusy(true);
    setSaveStatus("Saving…");
    try {
      setSettingsState(await api.saveSettings(settings));
      setSaveStatus("Saved.");
    } catch (e) {
      setSaveStatus(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  function setPrice(which: "flashPrice" | "proPrice", patch: Partial<{ input: string; cached: string; output: string }>) {
    const current = settings[which];
    const inputUsdPerMillion = patch.input !== undefined ? toNumber(patch.input) : (current?.inputUsdPerMillion ?? null);
    const cachedInputUsdPerMillion = patch.cached !== undefined ? toNumber(patch.cached) : (current?.cachedInputUsdPerMillion ?? null);
    const outputUsdPerMillion = patch.output !== undefined ? toNumber(patch.output) : (current?.outputUsdPerMillion ?? null);
    const price =
      inputUsdPerMillion == null && cachedInputUsdPerMillion == null && outputUsdPerMillion == null
        ? null
        : { inputUsdPerMillion: inputUsdPerMillion ?? 0, cachedInputUsdPerMillion, outputUsdPerMillion: outputUsdPerMillion ?? 0 };
    setSettingsState({ ...settings, [which]: price });
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center overflow-y-auto bg-ink/30 p-6">
      <div role="dialog" aria-modal="true" aria-labelledby="settings-title" className="flex w-full max-w-2xl flex-col gap-6 rounded-[32px] bg-card p-10 shadow-2xl">
        <div className="flex items-center justify-between">
          <h2 id="settings-title" className="m-0 font-display text-2xl font-semibold">Settings</h2>
          <button type="button" className={pillButton} onClick={onClose}>Close</button>
        </div>

        <div className="flex flex-col gap-3 border-b border-divider pb-6">
          <Label htmlFor="settings-api-key">Gemini API key</Label>
          <p className="m-0 text-muted">Personas and answers are written by Google Gemini. It is stored in Windows Credential Manager, never in project files.</p>
          <div className="flex items-center gap-3">
            <input id="settings-api-key" type="password" autoComplete="off" className={pillField} value={key} onChange={(e) => setKey(e.target.value)} placeholder={hasKey ? "Enter a new key to replace the stored one" : "Paste an API key from Google AI Studio"} />
          </div>
          <div className="flex items-center justify-between gap-4">
            <span role="status" className="text-sm text-muted">{keyStatus}</span>
            <div className="flex gap-3">
              {hasKey && <button type="button" className={pillButton} disabled={busy} onClick={testKey}>Test</button>}
              {hasKey && <button type="button" className={pillButton} disabled={busy} onClick={deleteKey}>Delete key</button>}
              <button type="button" className={primaryButton} disabled={!key.trim() || busy} onClick={saveKey}>Save and test</button>
            </div>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-x-6 gap-y-5 border-b border-divider pb-6">
          <div className="flex flex-col gap-2">
            <Label htmlFor="flash-model">Flash model</Label>
            <Help>Used for personas and answering. Automatic picks the newest stable Flash.</Help>
            <select id="flash-model" className={pillField} value={settings.flashModel ?? ""} onChange={(e) => setSettingsState({ ...settings, flashModel: e.target.value || null })}>
              <option value="">Automatic</option>
              {models.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="pro-model">Pro model</Label>
            <Help>Used for drafting, theme coding and synthesis. Automatic picks the newest stable Pro.</Help>
            <select id="pro-model" className={pillField} value={settings.proModel ?? ""} onChange={(e) => setSettingsState({ ...settings, proModel: e.target.value || null })}>
              <option value="">Automatic</option>
              {models.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
          </div>
        </div>

        <div className="flex flex-col gap-3 border-b border-divider pb-6">
          <Label htmlFor="usage-tier">Gemini usage tier</Label>
          <Help>Sets the request-rate limits the app stays under. Match this to your Google Cloud project's billing tier. Takes effect the next time the app launches.</Help>
          <select id="usage-tier" className={pillField} value={settings.usageTier} onChange={(e) => setSettingsState({ ...settings, usageTier: e.target.value as UsageTier })}>
            {USAGE_TIERS.map((t) => <option key={t.value} value={t.value}>{t.label}</option>)}
          </select>
        </div>

        <div className="flex flex-col gap-3">
          <Label>Gemini price table (USD per 1,000,000 tokens)</Label>
          <Help>
            From the Gemini pricing page. Once the Flash price is set, Step 3 estimates a run&apos;s cost and Step 4 shows it live. Cached input is the price for
            tokens Gemini serves from its cache (usually a tenth of input); left empty, they are charged at the input price.
          </Help>
          <div className="grid grid-cols-3 gap-x-6 gap-y-4">
            <div className="flex flex-col gap-2">
              <Label htmlFor="flash-input">Flash input</Label>
              <input id="flash-input" type="number" min={0} step="0.001" className={pillField} value={toText(settings.flashPrice?.inputUsdPerMillion)} onChange={(e) => setPrice("flashPrice", { input: e.target.value })} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="flash-cached">Flash cached input</Label>
              <input id="flash-cached" type="number" min={0} step="0.001" className={pillField} value={toText(settings.flashPrice?.cachedInputUsdPerMillion)} onChange={(e) => setPrice("flashPrice", { cached: e.target.value })} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="flash-output">Flash output</Label>
              <input id="flash-output" type="number" min={0} step="0.001" className={pillField} value={toText(settings.flashPrice?.outputUsdPerMillion)} onChange={(e) => setPrice("flashPrice", { output: e.target.value })} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="pro-input">Pro input</Label>
              <input id="pro-input" type="number" min={0} step="0.001" className={pillField} value={toText(settings.proPrice?.inputUsdPerMillion)} onChange={(e) => setPrice("proPrice", { input: e.target.value })} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="pro-cached">Pro cached input</Label>
              <input id="pro-cached" type="number" min={0} step="0.001" className={pillField} value={toText(settings.proPrice?.cachedInputUsdPerMillion)} onChange={(e) => setPrice("proPrice", { cached: e.target.value })} />
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="pro-output">Pro output</Label>
              <input id="pro-output" type="number" min={0} step="0.001" className={pillField} value={toText(settings.proPrice?.outputUsdPerMillion)} onChange={(e) => setPrice("proPrice", { output: e.target.value })} />
            </div>
          </div>
        </div>

        <div className="flex items-center justify-between gap-4">
          <span role="status" className="text-sm text-muted">{saveStatus}</span>
          <button type="button" className={primaryButton} disabled={busy} onClick={save}>Save settings</button>
        </div>
      </div>
    </div>
  );
}
