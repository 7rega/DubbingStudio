import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";

// Единый источник версии — tauri.conf.json (бампается при релизе). Прокидываем в бандл как __APP_VERSION__.
const appVersion = JSON.parse(readFileSync(new URL("../desktop/src-tauri/tauri.conf.json", import.meta.url), "utf-8")).version;

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: "./", // portable: assets load from any path (FastAPI-served / file://)
  server: { port: 5173 },
  define: { __APP_VERSION__: JSON.stringify(appVersion) },
});
