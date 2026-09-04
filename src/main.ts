import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import { createApp } from "vue";
import Kui from "kui-vue";
import "kui-vue/style/index.css";
import App from "./App.vue";
import "./styles/app.css";

document.documentElement.setAttribute("theme-mode", "dark");

createApp(App)
    .use(Kui)
    .provide("size", "small")
    .mount("#app");
