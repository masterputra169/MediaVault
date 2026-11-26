"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Archive, CircleHelp, HardDrive, LoaderCircle, Lock, LogOut, Settings2, X } from "lucide-react";
import { createContext, useCallback, useContext, useEffect, useState, type FormEvent, type ReactNode } from "react";
import { ApiError, api, message, setUnauthorizedHandler } from "@/lib/api";
import type { AuthSession, Health, Preferences, Settings, Theme } from "@/lib/types";
import { SettingsModal } from "./settings-modal";

const Context = createContext<{
  health: Health | null;
  checking: boolean;
  preferences: Preferences | null;
  reconnect: () => void;
  markOffline: () => void;
  notify: (text: string, error?: boolean) => void;
  setTheme: (theme: Theme) => void;
  openSettings: () => void;
} | null>(null);

export function useApp() {
  const app = useContext(Context);
  if (!app) throw new Error("AppShell is missing");
  return app;
}

export function AppShell({ children }: { children: ReactNode }) {
  const path = usePathname();
  const isSinglePage = path === "/";
  const [auth, setAuth] = useState<AuthSession | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [loginError, setLoginError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [health, setHealth] = useState<Health | null>(null);
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [checking, setChecking] = useState(true);
  const [revision, setRevision] = useState(0);
  const [theme, setTheme] = useState<Theme>("dark");
  const [toast, setToast] = useState<{ text: string; error: boolean } | null>(null);
  const reconnect = useCallback(() => { setChecking(true); setRevision((value) => value + 1); }, []);
  const markOffline = useCallback(() => setHealth(null), []);
  const notify = useCallback((text: string, error = false) => setToast({ text, error }), []);
  const openSettings = useCallback(() => setSettingsOpen(true), []);

  useEffect(() => {
    setUnauthorizedHandler(() => {
      setAuth({ auth_required: true, authenticated: false, username: null });
      setHealth(null);
      setSettingsOpen(false);
    });
    return () => setUnauthorizedHandler(null);
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    api<AuthSession>("/auth/session", { signal: controller.signal })
      .then((session) => { if (!controller.signal.aborted) setAuth(session); })
      .catch((error) => {
        if (controller.signal.aborted) return;
        if (error instanceof ApiError && (error.data.code === "NOT_FOUND" || error.data.code === "SERVER_OFFLINE")) {
          setAuth({ auth_required: false, authenticated: true, username: null });
        } else {
          setAuth({ auth_required: true, authenticated: false, username: null });
        }
      });
    return () => controller.abort();
  }, [revision]);

  useEffect(() => {
    if (!auth?.authenticated) return;
    const controller = new AbortController();
    api<Health>("/health", { signal: controller.signal })
      .then(setHealth)
      .catch(() => { if (!controller.signal.aborted) setHealth(null); })
      .finally(() => { if (!controller.signal.aborted) setChecking(false); });
    api<Settings>("/settings", { signal: controller.signal })
      .then((value) => {
        setPreferences(value.preferences);
        setTheme(value.preferences.theme);
      }).catch(() => {});
    return () => controller.abort();
  }, [auth?.authenticated, revision]);

  useEffect(() => { document.documentElement.dataset.theme = theme; }, [theme]);
  useEffect(() => {
    document.body.classList.toggle("single-page-body", isSinglePage && Boolean(auth?.authenticated));
    return () => document.body.classList.remove("single-page-body");
  }, [isSinglePage, auth?.authenticated]);
  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), toast.error ? 9500 : 4500);
    return () => window.clearTimeout(timer);
  }, [toast]);

  async function handleLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (submitting) return;
    setLoginError(null);
    setSubmitting(true);
    try {
      const session = await api<AuthSession>("/auth/login", {
        method: "POST",
        body: JSON.stringify({ username: username.trim(), password }),
      });
      setPassword("");
      setAuth(session);
      setChecking(true);
      setRevision((value) => value + 1);
    } catch (error) {
      setLoginError(message(error));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleLogout() {
    try {
      await api("/auth/logout", { method: "POST" });
    } catch {
      // Clear local session state even if network call fails.
    }
    setAuth({ auth_required: true, authenticated: false, username: null });
    setHealth(null);
    setSettingsOpen(false);
    setPassword("");
  }

  const healthLabel = checking ? "Memeriksa" : health?.status === "ok" ? "Server aktif"
    : health ? "Fitur terbatas" : "Server offline";

  if (!auth) {
    return (
      <main id="main" className="frame main login-shell">
        <div className="loading-text" role="status">
          <LoaderCircle size={18} className="spin" aria-hidden />
          <span>Memeriksa sesi...</span>
        </div>
      </main>
    );
  }

  if (auth.auth_required && !auth.authenticated) {
    return (
      <main id="main" className="frame main login-shell">
        <section className="panel login-card" aria-labelledby="login-heading">
          <div className="brand">
            <span className="brand-icon"><Lock aria-hidden size={18} /></span>
            <span>MediaVault<span className="brand-period">.</span></span>
          </div>
          <h1 id="login-heading">Masuk Pemilik</h1>
          <p>Akses privat khusus pemilik. Masukkan kredensial Anda untuk membuka MediaVault.</p>
          <form className="login-form" onSubmit={handleLogin}>
            <label>
              <span>Nama pengguna</span>
              <input
                type="text"
                name="username"
                autoComplete="username"
                required
                maxLength={64}
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                disabled={submitting}
              />
            </label>
            <label>
              <span>Kata sandi</span>
              <input
                type="password"
                name="password"
                autoComplete="current-password"
                required
                maxLength={256}
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                disabled={submitting}
              />
            </label>
            {loginError && <p className="field-error" role="alert">{loginError}</p>}
            <button type="submit" className="button primary" disabled={submitting || !username.trim() || !password}>
              {submitting ? <><LoaderCircle size={16} className="spin" aria-hidden /> Memeriksa...</> : "Masuk"}
            </button>
          </form>
        </section>
      </main>
    );
  }

  return (
    <Context.Provider value={{ health, checking, preferences, reconnect, markOffline, notify, setTheme, openSettings }}>
      <a className="skip-link" href="#main">Lewati navigasi</a>
      <header className="site-header">
        <div className="frame header-inner single-header">
          <Link href="/" className="brand" aria-label="MediaVault Dashboard">
            <span className="brand-icon"><Archive aria-hidden size={18} /></span>
            <span>MediaVault<span className="brand-period">.</span></span>
          </Link>
          <div className="header-actions">
            <button type="button" className="connection connection-btn" onClick={openSettings} title="Lihat kesehatan server & pengaturan">
              <span className={`indicator ${checking ? "waiting" : health?.status === "ok" ? "okay" : health ? "limited" : "offline"}`} />
              <span>{healthLabel}</span>
            </button>
            <button type="button" className="button small" onClick={openSettings} title="Pengaturan">
              <Settings2 size={14} aria-hidden />
              <span className="hide-xs">Pengaturan</span>
            </button>
            {auth.auth_required && (
              <button type="button" className="button small" onClick={handleLogout} title="Keluar dari sesi">
                <LogOut size={14} aria-hidden />
                <span className="hide-xs">Keluar</span>
              </button>
            )}
          </div>
        </div>
      </header>
      <main id="main" className={`frame ${isSinglePage ? "single-main" : "main"}`}>
        {!health && !checking && <div className="notice notice-error" role="status">
          <span>Backend tidak dapat dihubungi.</span>
          <button className="button small" onClick={reconnect}>Hubungkan ulang</button>
        </div>}
        {children}
      </main>
      <footer className="site-footer">
        <div className="frame footer-inner">
          <span><HardDrive size={14} aria-hidden /> MediaVault · Video, Audio &amp; Foto</span>
          <span className="hide-xs"><CircleHelp size={14} aria-hidden /> Unduh hanya media publik yang berhak Anda simpan.</span>
        </div>
      </footer>
      {settingsOpen && (
        <SettingsModal
          health={health}
          checking={checking}
          reconnect={reconnect}
          notify={notify}
          setTheme={setTheme}
          onClose={() => setSettingsOpen(false)}
          onSaved={setPreferences}
        />
      )}
      {toast && <div className={`toast ${toast.error ? "notice-error" : ""}`} role={toast.error ? "alert" : "status"}>
        <span>{toast.text}</span><button aria-label="Tutup notifikasi" className="icon-button" onClick={() => setToast(null)}><X size={17} /></button>
      </div>}
    </Context.Provider>
  );
}
