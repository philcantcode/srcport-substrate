"""Plugin surface: ModulePlugin, StepContext, PortBody."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field

from srcport_substrate import Artifact, ModuleManifest, WorkItem

from .presentation import Presentation, StepResult
from .storage import StoreWrite, TableSchema


@dataclass
class PortBody:
    port: str
    traits: dict[str, bytes] = field(default_factory=dict)
    entity_id: str = ""

    @staticmethod
    def with_trait(port: str, contract: str, body: bytes) -> PortBody:
        return PortBody(port=port, traits={contract: body})


@dataclass
class StepOutput:
    outputs: list[PortBody] = field(default_factory=list)


@dataclass
class StepContext:
    run_id: str
    work: WorkItem
    inputs: dict[str, Artifact] = field(default_factory=dict)
    _progress_buf: list[Presentation] = field(default_factory=list, repr=False)

    def emit_progress(self, presentation: Presentation) -> None:
        presentation.stage = presentation.stage  # keep
        from .presentation import PresentationStatus, StepStage

        presentation.stage = StepStage.PROGRESS
        if presentation.status == PresentationStatus.PENDING:
            presentation.status = PresentationStatus.RUNNING
        presentation.fill_identity(self.run_id, self.work)
        self._progress_buf.append(presentation)

    def take_progress(self) -> list[Presentation]:
        out = self._progress_buf
        self._progress_buf = []
        return out


class ModulePlugin(ABC):
    """Domain module as a host-side plugin."""

    @abstractmethod
    def manifest(self) -> ModuleManifest: ...

    @abstractmethod
    def execute(self, step: StepContext) -> StepOutput: ...

    def module_digest(self) -> str | None:
        return None

    def on_init(self, step: StepContext) -> Presentation | None:
        return None

    def on_final(self, step: StepContext, result: StepResult) -> Presentation | None:
        return None

    def storage_schema(self) -> TableSchema | None:
        return None

    def on_store(self, step: StepContext, result: StepResult) -> StoreWrite | None:
        return None
