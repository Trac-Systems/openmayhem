// rpc_server.mjs
import http from 'http'
import { applyCors } from './cors.js';
import { routes } from './routes/index.js';

export const createServer = (msbInstance, config) => {
  const server = http.createServer({}, async (req, res) => {
    
    // --- 1. Define safe 'respond' utility (Payload MUST be an object) ---
    const respond = (code, payload) => {
      // FIX: Prevent attempts to write headers if a response has already started
      if (res.headersSent) {
          console.warn(`Attempted to send response (Code: ${code}) after headers were already sent. payload:`, payload);
          return;
      }
      
      // Enforce JSON content type for all responses
      res.writeHead(code, { 'Content-Type': 'application/json' });
      
      // CRITICAL: Always JSON.stringify the input object
      res.end(JSON.stringify(payload)); 
    }

    // --- 2. Catch low-level request stream errors ---
    req.on('error', (err) => {
      console.error('Request stream error:', err);
      // Use the safe respond utility
      respond(500, { error: 'A stream-level request error occurred.' });
    });

    if (applyCors(req, res)) return;

    // Find the matching route
    let foundRoute = false;
    
    // Extract the path without query parameters
    const requestPath = req.url.split('?')[0];
    
    // Sort routes by path length (longest first) to ensure more specific routes match first
    const sortedRoutes = [...routes].sort((a, b) => b.path.length - a.path.length);
    
    for (const route of sortedRoutes) {
        // Exact path matching for base route, allow parameters after base path
        const routeBase = route.path.endsWith('/') ? route.path.slice(0, -1) : route.path;
        const requestParts = requestPath.split('/');
        const routeParts = routeBase.split('/');
        
        if (req.method === route.method && 
            requestParts.length >= routeParts.length &&
            routeParts.every((part, i) => part === requestParts[i])) {
            
            foundRoute = true;
            try {
                // This try/catch covers synchronous errors and errors from awaited promises
                // within the route.handler function.
                await route.handler({ req, res, respond,  msbInstance, config});
            } catch (error) {
                // Catch errors thrown directly from the handler (or its awaited parts)
                console.error(`Error on ${route.path}:`, error);
                respond(500, { error: 'An error occurred processing the request.' });
            }
            break;
        }
    }

    if (!foundRoute) {
        respond(404, { error: 'Not Found' });
    }
  });

  return server
}