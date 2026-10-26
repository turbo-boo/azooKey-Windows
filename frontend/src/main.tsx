import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";
import { SidebarProvider, SidebarTrigger } from "@/components/ui/sidebar"
import { ThemeProvider } from "@/components/theme-provider"
import { BrowserRouter, Routes, Route } from "react-router";
import { AppSidebar } from "@/components/app-sidebar"
import { ConfigGate } from "@/components/config-gate"

import { General } from "@/pages/general"
import { Appearance } from "@/pages/appearance"
import { Zenzai } from "@/pages/zenzai"
import { Dictionary } from "@/pages/dictionary"
import { About } from "@/pages/about"
import { Toaster } from "@/components/ui/sonner"

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SidebarProvider>
      <BrowserRouter>
        <AppSidebar />
        <main className="w-full p-6">
          {/* Below 768px the sidebar becomes a closed drawer, and Ctrl+B hides
              it on wider windows; this keeps the pages reachable either way. */}
          <SidebarTrigger className="-ml-1 mb-4" aria-label="メニューの表示を切り替え" />
          <ThemeProvider defaultTheme="system" storageKey="vite-ui-theme">
            <Routes>
              <Route path="/" element={<ConfigGate><General /></ConfigGate>} />
              <Route path="/appearance" element={<ConfigGate><Appearance /></ConfigGate>} />
              <Route path="/zenzai" element={<ConfigGate><Zenzai /></ConfigGate>} />
              <Route path="/dictionary" element={<ConfigGate><Dictionary /></ConfigGate>} />
              <Route path="/about" element={<About />} />
            </Routes>
            <Toaster />
          </ThemeProvider>
        </main>
      </BrowserRouter>
    </SidebarProvider>
  </React.StrictMode>,
);
