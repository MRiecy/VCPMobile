<script setup lang="ts">
// SensorCollector.vue
// Phase 2: Collects sensor data from Web APIs and pushes to Rust backend.
// Mounts alongside ToolInteractionOverlay, only active when distributed is connected.

import { ref, watch, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useDistributed } from "./composables/useDistributed";

const { status } = useDistributed();

// Timer handles for cleanup
const timers: ReturnType<typeof setInterval>[] = [];
const active = ref(false);

// ============================================================
// Sensor push helper
// ============================================================

async function pushSensor(key: string, value: string) {
  try {
    await invoke("update_sensor_data", { key, value });
  } catch (e) {
    console.warn(`[SensorCollector] Failed to push ${key}:`, e);
  }
}

// ============================================================
// Location (GPS)
// ============================================================

function startLocation() {
  if (!("geolocation" in navigator)) return;

  const collect = () => {
    navigator.geolocation.getCurrentPosition(
      (pos) => {
        const { latitude, longitude, accuracy, altitude } = pos.coords;
        const latDir = latitude >= 0 ? "N" : "S";
        const lonDir = longitude >= 0 ? "E" : "W";
        const lat = Math.abs(latitude).toFixed(4);
        const lon = Math.abs(longitude).toFixed(4);
        const acc = accuracy ? `${Math.round(accuracy)}m` : "N/A";
        const alt = altitude != null ? `${Math.round(altitude)}m` : "N/A";
        const value = `坐标: ${lat}°${latDir}, ${lon}°${lonDir} | 精度: ${acc} | 海拔: ${alt}`;
        pushSensor("location", value);
      },
      (err) => {
        pushSensor("location", `位置获取失败: ${err.message}`);
      },
      { enableHighAccuracy: true, timeout: 15000, maximumAge: 60000 }
    );
  };

  collect();
  timers.push(setInterval(collect, 120_000)); // 120s
}

// ============================================================
// Motion (Accelerometer / DeviceMotion)
// ============================================================

let motionHandler: ((e: DeviceMotionEvent) => void) | null = null;
let lastMotionPush = 0;
const MOTION_PUSH_INTERVAL = 30_000; // 30s

function startMotion() {
  // Accumulator for computing average acceleration
  let accSamples: number[] = [];

  motionHandler = (e: DeviceMotionEvent) => {
    const acc = e.accelerationIncludingGravity;
    if (acc && acc.x != null && acc.y != null && acc.z != null) {
      const magnitude = Math.sqrt(acc.x ** 2 + acc.y ** 2 + acc.z ** 2);
      accSamples.push(magnitude);
    }

    const now = Date.now();
    if (now - lastMotionPush >= MOTION_PUSH_INTERVAL && accSamples.length > 0) {
      lastMotionPush = now;
      const avg = accSamples.reduce((a, b) => a + b, 0) / accSamples.length;
      const max = Math.max(...accSamples);

      // Simple motion state detection
      let state = "静止";
      if (avg > 12) state = "运动中";
      else if (avg > 10.5) state = "步行中";
      else if (avg > 9.5) state = "轻微移动";

      const value = `状态: ${state} | 平均加速度: ${avg.toFixed(2)}m/s² | 峰值: ${max.toFixed(2)}m/s²`;
      pushSensor("motion", value);
      accSamples = [];
    }
  };

  window.addEventListener("devicemotion", motionHandler);
}

function stopMotion() {
  if (motionHandler) {
    window.removeEventListener("devicemotion", motionHandler);
    motionHandler = null;
  }
}

// ============================================================
// Ambient (Light / Barometer)
// ============================================================

let ambientLightSensor: any = null;
let ambientPressureSensor: any = null;
let lastLux = "N/A";
let lastPressure = "N/A";

function startAmbient() {
  // Ambient Light Sensor (Chrome 67+, requires secure context)
  try {
    if ("AmbientLightSensor" in window) {
      ambientLightSensor = new (window as any).AmbientLightSensor({ frequency: 1 });
      ambientLightSensor.addEventListener("reading", () => {
        const lux = ambientLightSensor.illuminance;
        if (lux != null) {
          let desc = "未知";
          if (lux < 50) desc = "暗";
          else if (lux < 200) desc = "室内";
          else if (lux < 1000) desc = "明亮";
          else desc = "户外";
          lastLux = `${Math.round(lux)} lux (${desc})`;
        }
      });
      ambientLightSensor.start();
    }
  } catch (e) {
    console.debug("[SensorCollector] AmbientLightSensor not available:", e);
  }

  // Barometer (Pressure Sensor)
  try {
    if ("PressureSensor" in window || "Barometer" in window) {
      const SensorClass = (window as any).PressureSensor || (window as any).Barometer;
      ambientPressureSensor = new SensorClass({ frequency: 1 });
      ambientPressureSensor.addEventListener("reading", () => {
        const pressure = ambientPressureSensor.pressure;
        if (pressure != null) {
          lastPressure = `${Math.round(pressure)} hPa`;
        }
      });
      ambientPressureSensor.start();
    }
  } catch (e) {
    console.debug("[SensorCollector] PressureSensor not available:", e);
  }

  // Push ambient data periodically
  const pushAmbient = () => {
    if (lastLux === "N/A" && lastPressure === "N/A") {
      pushSensor("ambient", "环境传感器: 设备不支持或权限未授予");
    } else {
      const parts: string[] = [];
      if (lastLux !== "N/A") parts.push(`环境光: ${lastLux}`);
      if (lastPressure !== "N/A") parts.push(`气压: ${lastPressure}`);
      pushSensor("ambient", parts.join(" | "));
    }
  };

  timers.push(setInterval(pushAmbient, 60_000)); // 60s
  // Initial push after 5s to let sensors warm up
  const initTimer = setTimeout(pushAmbient, 5000) as unknown as ReturnType<typeof setInterval>;
  timers.push(initTimer);
}

function stopAmbient() {
  try { ambientLightSensor?.stop(); } catch (_) {}
  try { ambientPressureSensor?.stop(); } catch (_) {}
  ambientLightSensor = null;
  ambientPressureSensor = null;
}

// ============================================================
// Lifecycle: start/stop based on distributed connection status
// ============================================================

function startAll() {
  if (active.value) return;
  active.value = true;
  console.log("[SensorCollector] Starting sensor collection");
  startLocation();
  startMotion();
  startAmbient();
}

function stopAll() {
  if (!active.value) return;
  active.value = false;
  console.log("[SensorCollector] Stopping sensor collection");
  timers.forEach(clearInterval);
  timers.length = 0;
  stopMotion();
  stopAmbient();
}

watch(
  () => status.value.connected,
  (connected) => {
    if (connected) {
      startAll();
    } else {
      stopAll();
    }
  },
  { immediate: true }
);

onUnmounted(() => {
  stopAll();
});
</script>

<template>
  <!-- Headless component — no UI -->
</template>
