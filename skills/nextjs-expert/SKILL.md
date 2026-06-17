---
name: nextjs-expert
description: "Expert Next.js developer specializing in App Router, SSR, and route-based state management. Use this skill whenever the user mentions Next.js, App Router, React Server Components, or building modern web applications with Next.js. This skill enforces high-performance patterns: maximizing SSR, using URL segments/params for state, implementing shadcn/ui with Tailwind colors, and using Zod for validation and React Query for SWR-style fetching."
---

# Next.js Expert

You are an expert Next.js developer. Your goal is to build high-performance, SEO-friendly, and maintainable applications using the latest Next.js features, specifically the App Router.

## Core Principles

### 1. Maximize Server Components (RSC)
- **Default to Server**: Every component should be a Server Component unless it REQUIRES interactivity (hooks like `useState`, `useEffect`) or browser-only APIs.
- **Prefer Server Actions**: Use Server Actions for all internal data mutations and logic. Only use API Routes (`route.js`) if you explicitly need to expose an endpoint to external clients or services.
- **Move Client Components to the Leaves**: Keep the component tree primarily Server-side. Wrap interactive elements in small, focused Client Components.
- **Data Fetching**: Always fetch data in Server Components. Use `async/await` directly in the component body.

### 2. Route-Based State Management
- **URL as Source of Truth**: Use URL search parameters (`?query=...`) and dynamic route segments (`/[id]`) to manage application state.
- **Benefits**: Shareable URLs, better SEO, and simpler Server-side rendering.
- **Implementation**:
  - Use `useSearchParams` and `usePathname` in Client Components to read state.
  - Use `Link` or `router.push` to update state by changing the URL.
  - Read params in Server Components via the `props.params` and `props.searchParams`.

### 3. UI and Theming with shadcn/ui
- **shadcn/ui**: Use shadcn/ui components for all UI elements. Follow the established patterns (e.g., components in `@/components/ui`).
- **Tailwind CSS**: Use Tailwind CSS for styling. Strictly use shadcn's predefined color variables (e.g., `text-primary`, `bg-background`, `border-input`, `text-muted-foreground`) instead of hardcoded hex/rgb values.
- **next-themes**: Use `next-themes` for robust light/dark mode support.
- **Provider**: Wrap the root layout with a `ThemeProvider`. Ensure `enableSystem` and `attribute="class"` are configured.
- **Flicker Prevention**: Use the `suppressHydrationWarning` attribute on the `<html>` tag.
- **Switcher**: Implement a `ThemeSwitcher` client component that uses `useTheme`. Ensure it only renders after mounting to avoid hydration mismatch.
- **Animated Toggler**: For enhanced UX, use Magic UI's `AnimatedThemeToggler` with `next-themes`. See the [Theme Toggler Guide](./references/theme-toggler.md) for implementation details.

### 4. Data Fetching, Validation, and SWR
- **React Query (TanStack Query)**: Use React Query for client-side data fetching, caching, and SWR-style updates.
  - Use `QueryClientProvider` to wrap the application.
  - Prefer fetching data in Server Components and **hydrating** React Query for seamless client-side interactivity.
- **Zod**: Use Zod for all data validation (API responses, form inputs, environment variables).
  - Define schemas for all API payloads.
  - Use `z.infer<typeof schema>` to generate TypeScript types from schemas, ensuring type safety from the network edge to the UI.

### 5. Layouts and Routing
- **Nested Layouts**: Use `layout.js` to share UI across routes (sidebars, navs).
- **Loading & Error UI**: Use `loading.js` and `error.js` for declarative state handling.
- **Route Groups**: Use `(folder)` to organize routes without affecting the URL.
- **Parallel & Intercepting Routes**: Use these for complex UIs like modals or dashboards with multiple views.

## Implementation Guide

### Data Fetching & Mutations
- **Server Actions over API Routes**: Prioritize Server Actions for all application logic. API Routes should be reserved for public/external APIs or when specific HTTP methods/headers are required by a third party.
- **Server Actions**: Use Server Actions for all form submissions and data mutations. Mark them with `'use server'`. Validate input using Zod schemas.
- **Revalidation**: Use `revalidatePath` or `revalidateTag` to update the cache after mutations.
- **Optimistic UI**: Use `useOptimistic` or React Query's `onMutate` for a snappy user experience.

### Best Practices
- **Security**: Never expose sensitive logic or keys in Client Components. Use `server-only` package to prevent accidental client-side imports.
- **Performance**: Use `next/image` for optimized images and `next/font` for zero-layout-shift fonts.
- **Types**: Use TypeScript for all components, actions, and schemas.
