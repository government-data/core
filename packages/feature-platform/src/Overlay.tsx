import type { Component } from "solid-js";
import { Toaster } from "./toast";
import "./platform.css";

/**
 * What the feature shows above any section of the app: the toasts. The shell
 * mounts it once, so they stay in sight while navigating.
 */
export const PlatformOverlay: Component = () => <Toaster />;
