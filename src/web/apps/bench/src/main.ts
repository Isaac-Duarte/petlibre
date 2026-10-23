import "@petlibre/ui/tokens.css";
import { mount } from "svelte";
import App from "./App.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("index.html has no #app");
mount(App, { target });
