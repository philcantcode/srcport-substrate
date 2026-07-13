"""Step lifecycle presentation data (srcport.ui.v1)."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any

from srcport_substrate import NamedArtifact, WorkItem

CONTRACT_STEP_INIT = "srcport.ui.v1.StepInit"
CONTRACT_STEP_PROGRESS = "srcport.ui.v1.StepProgress"
CONTRACT_STEP_FINAL = "srcport.ui.v1.StepFinal"
CONTRACT_STEP_SKIPPED = "srcport.ui.v1.StepSkipped"
CONTRACT_STEP_CACHED = "srcport.ui.v1.StepCached"


class StepStage(str, Enum):
    INIT = "init"
    PROGRESS = "progress"
    FINAL = "final"
    SKIPPED = "skipped"
    CACHED = "cached"

    def contract_ref(self) -> str:
        return {
            StepStage.INIT: CONTRACT_STEP_INIT,
            StepStage.PROGRESS: CONTRACT_STEP_PROGRESS,
            StepStage.FINAL: CONTRACT_STEP_FINAL,
            StepStage.SKIPPED: CONTRACT_STEP_SKIPPED,
            StepStage.CACHED: CONTRACT_STEP_CACHED,
        }[self]


class PresentationStatus(str, Enum):
    PENDING = "pending"
    RUNNING = "running"
    BLOCKED = "blocked"
    OK = "ok"
    EMPTY = "empty"
    FAILED = "failed"


@dataclass
class Presentation:
    stage: StepStage = StepStage.PROGRESS
    title: str = ""
    status: PresentationStatus = PresentationStatus.RUNNING
    detail: str | None = None
    progress: float | None = None
    run_id: str = ""
    work_id: str = ""
    node_id: str = ""
    module: str = ""
    capability: str = ""
    phase: str | None = None
    highlight_ports: list[str] = field(default_factory=list)
    output_ports: list[str] = field(default_factory=list)
    meta: dict[str, str] = field(default_factory=dict)

    @staticmethod
    def init(title: str) -> Presentation:
        return Presentation(stage=StepStage.INIT, title=title, status=PresentationStatus.RUNNING)

    @staticmethod
    def at_progress(title: str, fraction: float | None = None) -> Presentation:
        """Build a Progress-stage presentation (avoids clashing with the progress field)."""
        return Presentation(
            stage=StepStage.PROGRESS,
            title=title,
            status=PresentationStatus.RUNNING,
            progress=fraction,
        )

    @staticmethod
    def final_ok(title: str) -> Presentation:
        return Presentation(
            stage=StepStage.FINAL, title=title, status=PresentationStatus.OK, progress=1.0
        )

    @staticmethod
    def final_failed(title: str, detail: str) -> Presentation:
        return Presentation(
            stage=StepStage.FINAL,
            title=title,
            status=PresentationStatus.FAILED,
            detail=detail,
        )

    @staticmethod
    def skipped(title: str, detail: str) -> Presentation:
        return Presentation(
            stage=StepStage.SKIPPED,
            title=title,
            status=PresentationStatus.EMPTY,
            detail=detail,
            progress=1.0,
        )

    @staticmethod
    def cached(title: str, detail: str) -> Presentation:
        return Presentation(
            stage=StepStage.CACHED,
            title=title,
            status=PresentationStatus.OK,
            detail=detail,
            progress=1.0,
        )

    def with_detail(self, detail: str) -> Presentation:
        self.detail = detail
        return self

    def with_phase(self, phase: str) -> Presentation:
        self.phase = phase
        return self

    def fill_identity(self, run_id: str, work: WorkItem) -> None:
        if not self.run_id:
            self.run_id = run_id
        if not self.work_id:
            self.work_id = work.id
        if not self.node_id:
            self.node_id = work.node_id
        if not self.module:
            self.module = work.module
        if not self.capability:
            self.capability = work.capability

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "stage": self.stage.value,
            "title": self.title,
            "status": self.status.value,
        }
        if self.detail is not None:
            d["detail"] = self.detail
        if self.progress is not None:
            d["progress"] = self.progress
        if self.run_id:
            d["run_id"] = self.run_id
        if self.work_id:
            d["work_id"] = self.work_id
        if self.node_id:
            d["node_id"] = self.node_id
        if self.module:
            d["module"] = self.module
        if self.capability:
            d["capability"] = self.capability
        if self.phase is not None:
            d["phase"] = self.phase
        if self.highlight_ports:
            d["highlight_ports"] = self.highlight_ports
        if self.output_ports:
            d["output_ports"] = self.output_ports
        if self.meta:
            d["meta"] = self.meta
        return d


@dataclass
class StepResult:
    ok: bool
    outputs: list[NamedArtifact] = field(default_factory=list)
    error: str | None = None


@dataclass
class StepEvent:
    stage: StepStage
    presentation: Presentation
    artifact_id: str = ""
