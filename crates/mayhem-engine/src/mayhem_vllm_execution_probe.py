"""Read-only named worker RPC; importing this module has no runtime side effects."""


class MayhemExecutionProbe:
    def mayhem_execution_snapshot(self):
        import os

        # This is the GPU worker's resolved config, not the frontend copy.
        compilation = self.compilation_config
        mode = compilation.mode
        graphs = compilation.cudagraph_mode
        return {
            "rank": self.rank,
            "local_rank": self.local_rank,
            "world_size": self.parallel_config.world_size,
            "pid": os.getpid(),
            "compilation_mode": getattr(mode, "value", mode),
            "cudagraph_mode": getattr(graphs, "name", graphs),
        }
