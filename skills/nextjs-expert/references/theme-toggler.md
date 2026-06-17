# Theme Toggler with next-themes and Magic UI

This guide explains how to implement an animated theme toggler using `next-themes` and the `AnimatedThemeToggler` component from Magic UI.

## Overview

Combining `next-themes` with Magic UI's `AnimatedThemeToggler` provides a smooth, visually appealing transition between light and dark modes in a Next.js application.

## Prerequisites

1.  **Install next-themes**:
    ```bash
    npm install next-themes
    ```
2.  **Magic UI AnimatedThemeToggler**:
    Install the component using the shadcn CLI:
    ```bash
    pnpm dlx shadcn@latest add @magicui/animated-theme-toggler
    ```
    Ensure the component is correctly registered (typically at `@/registry/magicui/animated-theme-toggler` or `@/components/magicui/animated-theme-toggler`).

## Implementation Steps

### 1. Set up ThemeProvider

Ensure your application is wrapped in a `ThemeProvider`. This should typically be done in your root layout or a dedicated provider component.

```tsx
// components/theme-provider.tsx
"use client"

import * as React from "react"
import { ThemeProvider as NextThemesProvider } from "next-themes"
import { type ThemeProviderProps } from "next-themes/dist/types"

export function ThemeProvider({ children, ...props }: ThemeProviderProps) {
  return <NextThemesProvider {...props}>{children}</NextThemesProvider>
}
```

### 2. Create the Theme Toggle Component

Create a client component that bridges `next-themes` and the `AnimatedThemeToggler`.

```tsx
"use client"

import * as React from "react"
import { useTheme } from "next-themes"
import { AnimatedThemeToggler } from "@/registry/magicui/animated-theme-toggler"

export function ThemeToggle() {
  const { resolvedTheme, setTheme } = useTheme()
  const [mounted, setMounted] = React.useState(false)

  // Avoid hydration mismatch by only rendering after mount
  React.useEffect(() => {
    setMounted(true)
  }, [])

  if (!mounted) {
    // Return a placeholder or null to avoid layout shift
    return <div className="h-9 w-9" />
  }

  return (
    <div className="flex justify-center p-6">
      <AnimatedThemeToggler
        theme={resolvedTheme === "dark" ? "dark" : "light"}
        onThemeChange={setTheme}
      />
    </div>
  )
}
```

### 3. Integrate into Layout

Add the `ThemeToggle` to your navigation bar or header.

```tsx
import { ThemeToggle } from "@/components/theme-toggle"

export function Navbar() {
  return (
    <nav className="flex items-center justify-between p-4">
      <div className="font-bold">My App</div>
      <ThemeToggle />
    </nav>
  )
}
```

## Best Practices

- **Hydration Safety**: Use the `mounted` state pattern to prevent the server-rendered HTML from mismatching the client-rendered theme state.
- **resolvedTheme**: Always use `resolvedTheme` from `useTheme()` instead of `theme`. `resolvedTheme` correctly handles the "system" setting, reflecting whether the user's OS is currently in dark or light mode.
- **Accessibility**: Ensure the toggler is keyboard accessible and has appropriate aria-labels if not already provided by the Magic UI component.
