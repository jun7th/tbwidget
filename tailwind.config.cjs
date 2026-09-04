/**
 * 可选 Tailwind 配置。
 * 默认构建不依赖 Tailwind；基础组件统一使用 kui-vue。
 * 若业务页面需要 Tailwind，可自行安装 tailwindcss/postcss/autoprefixer 后启用。
 */
module.exports = {
  content: ["./index.html", "./src/**/*.{vue,js,ts}", "./plugins/**/*.{html,js}"],
  theme: { extend: {} },
  plugins: [],
};
