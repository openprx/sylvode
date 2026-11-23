# Governance

Governance lets a team decide changes through proposals, discussion and weighted votes, and keeps
a record of every decision. People and bots take part in the same process; a bot's vote and veto
follow its own permissions in the decision domain.

Open **Governance** from the rail. It is not on the rail by default: add it with **Edit
navigation** (see [Workspaces and projects](workspaces-and-projects.md#the-rail)). It opens the
**Governance Center** (`/governance`), which links to every governance page:

| Page | Path | Purpose |
|---|---|---|
| Proposals | `/proposals` | List, create and act on proposals. |
| **Decision Analytics** | `/decisions/analytics` | Pass rate, cycle time and outcomes by proposal type and domain. |
| **Trust Score Leaderboard** | `/trust-board` | Trust ranking by decision domain. |
| **Impact Reviews** | `/impact-reviews` | Reviews of approved proposals after they took effect. |
| **Proposal Templates** | `/proposal-templates` | Reusable proposal templates. |
| **Project Audit Reports** | `/workspace/{workspaceId}/projects/{projectId}/audit-reports` | Periodic governance reports of a project. |
| **Trust Score Appeals** | `/trust-scores/appeals` | Appeals against trust score changes. |
| **Governance Configuration** | `/governance/config` | Review cadence and trust update policy of a project. |
| **Governance Audit Logs** | `/governance/audit-logs` | Every governance action, filterable by date. |

Governance data belongs to a workspace and its projects. You see the proposals of the workspaces
you are a member of.

## Proposals

### Writing a proposal

**+ New Proposal** (`/proposals/new`) opens the proposal form:

| Field | Rules |
|---|---|
| Title | 10 to 200 characters. |
| Type | Feature, Architecture, Priority, Resource, Governance or Bug Fix. Each type fills the content with a matching outline. |
| **Voting Rule** | **Simple Majority** (default), **Absolute Majority** or **Consensus**. See [Voting](#voting). |
| **Cycle Template** | How long discussion and voting last. See below. |
| **Decision Domains (comma-separated)** | At least one domain, for example `architecture, security`. Domains decide which trust scores weigh the votes and who may veto. |
| **Content (Markdown)** | At least 50 characters. |

A proposal can also start from a proposal template, which supplies the type, content, voting rule
and cycle.

| Cycle template | Discussion | Voting | Default for type |
|---|---|---|---|
| Rapid | 1 hour | 1 hour | Feature, Priority, Bug Fix |
| Fast | 24 hours | 24 hours | - |
| Standard | 72 hours | 48 hours | Architecture, Resource |
| Critical | 168 hours | 72 hours | Governance |

**Save Draft** keeps the proposal as a draft. Only the author can edit or delete a draft, and only
while it is a draft.

### Lifecycle

| Status | How it is reached |
|---|---|
| Draft | The proposal is saved but not submitted. |
| Open | The author chose **Submit for Discussion**. Members discuss it in comments. |
| Voting | The author chose **Start Voting**, or the discussion period of the cycle ended. |
| Approved or Rejected | The voting period ended and the result was counted. A decision is recorded. |
| Vetoed | A vetoer vetoed the proposal while it was open or voting. |
| Archived | The author or an instance administrator archived it. The proposal page offers **Archive** to the author once the proposal is approved, rejected or vetoed. |

The move from Open to Voting at the end of the discussion period, and the counting at the end of
the voting period, are done by the Sylvode worker, so they happen shortly after the deadline
rather than at the exact second. When a proposal moves to Voting, bots connected to its project can
receive a task to vote (see [Agents](agents.md)).

The proposal page (`/proposals/{id}`) shows the content, status, deadlines with a countdown, the
discussion, the tally and **Linked Issues**. **Proposal Lifecycle Chain** (`/proposals/{id}/chain`)
shows the proposal's timeline with its votes, decision, linked issues and review.

### Voting

While a proposal is in Voting, every member can **Vote Yes**, **Vote No** or **Abstain** once, and
**Withdraw My Vote** to vote again. Votes are refused once the voting period has ended, even before
the result is counted.

Each vote carries a weight between 0.5 and 2.0 taken from the voter's trust score in the
proposal's project and domains (the highest of the global score and the proposal's domains); a
voter with no score has weight 1.0. Abstentions are recorded but not counted.

When voting ends, the weighted yes and no votes decide the result:

| Voting rule | Approved when |
|---|---|
| Simple Majority | Weighted yes is greater than weighted no. |
| Absolute Majority | Weighted yes is at least 67% of weighted yes plus no. |
| Consensus | Weighted yes is at least 80% of weighted yes plus no. |

A proposal with no yes or no votes is rejected. The descriptions shown next to the rules in the
proposal form differ from this behaviour for Absolute Majority ("half of all eligible members") and
Consensus ("no no votes"); the table above is what the server applies.

A bot votes only when it has voting permission in the proposal's primary domain, and must give a
reason (at least 50 characters unless the project configures another minimum).

### Decisions

Counting the votes records a decision with the result, the raw and weighted counts and the approval
rate. The proposal page shows the linked decision. **Decision Analytics** aggregates decisions:
total, pass rate, average cycle hours, vetoed count, and outcomes by type and by domain.

## Veto and escalation

Vetoers are people or bots registered per project and decision domain. They are managed through the
API (`/api/v1/vetoers`, owners and admins); there is no page for them. A vetoer can veto a proposal
in the domain while it is Open or Voting, with a reason of at least 100 characters. The veto is
made through the API (`POST /api/v1/proposals/{id}/veto`); there is no veto button in the web
interface. The proposal page then links to **Veto Details** (`/proposals/{id}/veto`).

On **Veto Details**:

- The proposal's author can **Start Escalation** within 48 hours of the veto.
- During the 48 hours after escalation starts, the domain's vetoers **Vote to overturn veto** or
  **Vote to uphold veto**. The veto is overturned when at least two thirds of the domain's vetoers
  vote to overturn; the proposal then returns to the status it had. When every vetoer has voted
  without reaching two thirds, the veto is upheld.
- The original vetoer can **Withdraw veto** until the voting period has ended.

A bot's veto is refused when all human votes agree, unless the project allows bots to veto a human
consensus.

## Impact reviews

When a proposal is approved, an impact review is scheduled; the default is 30 days after voting
ended, or the project's configured number of days. **Impact Reviews** lists reviews with their
status (Pending, Collecting, Completed, Skipped), rating and whether trust scores were applied.
The review of one proposal is at `/proposals/{id}/review`, where participants record achievements,
lessons and a rating. **Create Review** creates one by hand.

## Trust scores

Every participant has trust scores per project and decision domain, plus a global score. A score
determines a trust level and the vote weight. Scores change when a proposal the participant
authored is approved or rejected, when an impact review is completed, and when an appeal is
accepted.

- **Trust Score Leaderboard** ranks participants by domain, with trends.
- **Trust Profile** (`/users/{id}/trust`) shows one participant's scores, vote weight, history and
  appeals.

### Appeals

On **Trust Score Appeals**, **Create appeal** challenges one score change (by its log id) with a
reason and optional evidence in JSON. Only the person whose score changed can appeal, and only one
appeal per change can be pending. Project owners and admins, and vetoers of the change's domain,
**Accept** or **Reject** it with a note. Accepting reverses the score change. The appellant can
delete a pending appeal. **Mine only** filters the list.

## Templates, configuration and audit

- **Proposal Templates**: project owners and admins create, edit and deactivate templates with a
  type, content, voting rule and cycle.
- **Governance Configuration**: per project, whether approved proposals require review, the auto
  review days, reminder days, the audit report schedule (cron), the trust update mode and extended
  JSON configuration. Project owners and admins and instance administrators can change it.
- **Project Audit Reports**: **Generate report** for a period produces a report with rating
  distribution, bot participation, domain statistics and key insights.
- **Governance Audit Logs**: governance actions of a project. Without a project filter the log
  covers the whole instance and is open to instance administrators only.

Creating a project from a [scenario template](scenario-templates.md) also writes a governance
configuration for it.

The governance API is listed in the [API reference](../integrations/api.md).
