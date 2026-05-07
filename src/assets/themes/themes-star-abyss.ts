export const meta = {
  fileName: 'themes-star-abyss.css',
  name: '星渊雪境',
};

export const variables = {
  dark: {
  "--chat-wallpaper-dark": "'themes_star_abyss_dark.jpg'",
  "--primary-bg": "#101014",
  "--secondary-bg": "#1a1a1e",
  "--tertiary-bg": "#0d0d10",
  "--accent-bg": "#28282c",
  "--border-color": "#3a3a3e",
  "--input-bg": "#1e1e22",
  "--panel-bg-dark": "rgba(26, 26, 30, 0.75)",
  "--primary-text": "#e0e0e0",
  "--secondary-text": "#a4a4a4",
  "--highlight-text": "#FFB74D",
  "--text-on-accent": "#ffffff",
  "--placeholder-text": "#6a6a6e",
  "--quoted-text": "#5D9CEC",
  "--user-text": "#ffffff",
  "--agent-text": "#e0e0e0",
  "--user-bubble-bg": "rgba(255, 183, 77, 0.15)",
  "--assistant-bubble-bg": "rgba(255, 255, 255, 0.08)",
  "--button-bg": "#FFB74D",
  "--button-hover-bg": "#ffaa33",
  "--danger-color": "#e57373",
  "--success-color": "#66bb6a",
  "--tool-bubble-bg": "rgba(58, 58, 62, 0.15)",
  "--tool-bubble-border": "#FFB74D",
  "--shimmer-color-transparent": "rgba(255, 183, 77, 0.2)",
  "--shimmer-color-highlight": "rgba(255, 183, 77, 0.6)",
  "--panel-text-shadow": "0 1px 2px rgba(0, 0, 0, 0.4)",
  "--scrollbar-track": "rgba(255, 255, 255, 0.15)",
  "--scrollbar-thumb": "rgba(255, 183, 77, 0.6)",
  "--scrollbar-thumb-hover": "rgba(255, 183, 77, 0.8)",
  "--panel-bg": "var(--panel-bg-dark)"
},
  light: {
  "--chat-wallpaper-light": "'themes_snow_realm_light.jpg'",
  "--primary-bg": "#f4f6f8",
  "--secondary-bg": "#ffffff",
  "--tertiary-bg": "#e9edf0",
  "--accent-bg": "#e0e6eb",
  "--border-color": "#e0e6eb",
  "--input-bg": "#ffffff",
  "--panel-bg-light": "rgba(255, 255, 255, 0.9)",
  "--primary-text": "#2c3e50",
  "--secondary-text": "#5a6f80",
  "--highlight-text": "#5D9CEC",
  "--text-on-accent": "#ffffff",
  "--placeholder-text": "#a0aab3",
  "--quoted-text": "#007bff",
  "--user-text": "#2c3e50",
  "--agent-text": "#2c3e50",
  "--user-bubble-bg": "rgba(93, 156, 236, 0.1)",
  "--assistant-bubble-bg": "rgba(255, 255, 255, 0.8)",
  "--button-bg": "#5D9CEC",
  "--button-hover-bg": "#4a89dc",
  "--danger-color": "#e74c3c",
  "--success-color": "#4caf50",
  "--tool-bubble-bg": "rgba(224, 230, 235, 0.5)",
  "--tool-bubble-border": "#5D9CEC",
  "--shimmer-color-transparent": "rgba(44, 62, 80, 0.08)",
  "--shimmer-color-highlight": "rgba(44, 62, 80, 0.15)",
  "--scrollbar-track": "rgba(0, 0, 0, 0.1)",
  "--scrollbar-thumb": "rgba(93, 156, 236, 0.6)",
  "--scrollbar-thumb-hover": "rgba(93, 156, 236, 0.8)",
  "--panel-bg": "var(--panel-bg-light)"
},
};

export const extraCss = `
.tool-bubble {    border: 1px solid var(--tool-bubble-border);    background: var(--tool-bubble-bg);}
`;
