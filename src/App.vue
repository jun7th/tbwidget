<script setup lang="ts">
import PluginPopup from "./components/PluginPopup.vue";
import PluginSettings from "./components/PluginSettings.vue";
import SettingsView from "./views/SettingsView.vue";
import TaskbarView from "./views/TaskbarView.vue";
import { windowMode } from "./app/windowMode";
</script>

<template>
  <PluginPopup v-if="windowMode.pluginPopup" />
  <PluginSettings v-else-if="windowMode.pluginSettings" />
  <SettingsView v-else-if="windowMode.settings" />
  <TaskbarView v-else />
</template>
