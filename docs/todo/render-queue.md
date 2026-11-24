# The render queue

**When:** next.

**Why:** the Export page adds a job to the queue and starts it at once. The owner chose the held queue of the Deliver page (2026-11-24).

**What is measured** ([spec §10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212)):

- Add to Render Queue adds a held job and selects it. Render 1 renders the selected job. Render All renders all jobs from the top down.
- A completed job shows "Completed in" and its time. A second render of it asks first ("Re-Render Jobs?").
- After Clear Render Status, a render over an existing file asks first ("Replace Existing File?").
- The X removes a job with no prompt. A drag moves a job. Job names follow the position.
- Show Job Details, Clear Rendered and Clear All are in the "..." menu of the queue.
- A cancelled job stays in the queue after a reopen of the project.

**How:**

- The queue goes in the project file, with the timeline and a fixed range of each job.
- The CLI, IPC and MCP add, render, remove, move and clear jobs.

**Open:**

- A pick of Entire Timeline removes the marks in the Deliver page. vidcut keeps them until a measurement tells whether those marks are the marks of the Cut page.
