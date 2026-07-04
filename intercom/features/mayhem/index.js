import Feature from 'trac-peer/src/artifacts/feature.js';

class MayhemFeature extends Feature {
  async record(key, value) {
    await this.append(key, value);
  }

  async start() {}

  async stop() {}
}

export default MayhemFeature;
