--- Shared constants for the Poste plugin.
---
--- Centralizes all hardcoded values (timing, animation, paths, etc.)
--- so they can be tuned in one place.
return {
  -- Spinner animation
  SPINNER_FRAMES = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" },
  SPINNER_INTERVAL_MS = 100,

  -- Debounce intervals (ms)
  CURSOR_MOVED_DEBOUNCE_MS = 100,
  SYNTAX_REFRESH_DEBOUNCE_MS = 150,

  -- Completion
  BLINK_SCORE_OFFSET = 1000,

  -- Binary search paths (CWD-relative, checked in order)
  BINARY_CWD_PATHS = { "./target/debug/poste", "./target/release/poste" },

  -- HTTP history
  HTTP_HISTORY_MAX = 100,

  -- Conflict resolution: max suffix attempts before overwriting
  MAX_CONFLICT_SUFFIX = 1000,

  -- Indicator namespace name
  INDICATOR_NS_NAME = "poste_indicator",
  SIGN_GROUP_NAME = "poste_sg_4a7f",
}
