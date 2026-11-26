import type { Metadata } from "next";
import { AppShell } from "@/components/app-shell";
import "./globals.css";

export const metadata: Metadata = {
  title: { default: "MediaVault", template: "%s · MediaVault" },
  description: "Unduh video, audio, dan foto publik (MP4, MP3/M4A, JPG/PNG/WebP/ZIP) dengan aman.",
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="id" data-theme="dark" suppressHydrationWarning>
      <body><AppShell>{children}</AppShell></body>
    </html>
  );
}
