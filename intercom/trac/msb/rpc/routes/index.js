import { v1Routes } from './v1.js';

// future version
// import { v2Routes } from './v2.mjs';

// Map each version to its route list
const routeMap = {
  v1: v1Routes,
  // v2: v2Routes
};

// Dynamically collect supported versions from routeMap
const supportedVersions = Object.keys(routeMap);

// Helper to prefix all routes with version
function prefixRoutes(prefix, routes) {
  return routes.map(route => ({
    ...route,
    path: `/${prefix}${route.path}`,
  }));
}

// Final route export
export const routes = [
  // Prefix each version’s routes automatically
  ...supportedVersions.flatMap(v => prefixRoutes(v, routeMap[v])),
];
