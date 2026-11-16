# Third-party notices

Several skills and roles in RUI are adapted from public open-source work. Each is listed below with its source, license and copyright line.

**Modification notice (Apache-2.0 section 4(b)).** Every adapted file listed here was modified by RUI: rewritten for RUI's pod setup (members ask their Lead with `ask_lead` and report with `report_status`; there is no human partner mid-task), shortened, and merged or split where noted. Each adapted file also ends with a line naming its source and stating that it was modified.

Nothing here is copied from CC BY-SA or MPL sources.

## Adapted skills

| File | Source | License and copyright |
|---|---|---|
| `test-driven-development` | https://github.com/obra/superpowers/blob/main/skills/test-driven-development/SKILL.md | MIT — Copyright (c) 2025 Jesse Vincent |
| `systematic-debugging` | https://github.com/obra/superpowers/blob/main/skills/systematic-debugging/SKILL.md (+ root-cause-tracing.md, defense-in-depth.md, condition-based-waiting.md) | MIT — Copyright (c) 2025 Jesse Vincent |
| `verification-before-completion` | https://github.com/obra/superpowers/blob/main/skills/verification-before-completion/SKILL.md | MIT — Copyright (c) 2025 Jesse Vincent |
| `writing-plans` | https://github.com/obra/superpowers/blob/main/skills/writing-plans/SKILL.md | MIT — Copyright (c) 2025 Jesse Vincent |
| `executing-plans` | https://github.com/obra/superpowers/blob/main/skills/executing-plans/SKILL.md | MIT — Copyright (c) 2025 Jesse Vincent |
| `reporting-status` | https://github.com/obra/superpowers/blob/main/skills/subagent-driven-development/SKILL.md (+ implementer-prompt.md, references/handling-reports.md) | MIT — Copyright (c) 2025 Jesse Vincent |
| `receiving-code-review` | https://github.com/obra/superpowers/blob/main/skills/receiving-code-review/SKILL.md | MIT — Copyright (c) 2025 Jesse Vincent |
| `code-review` | https://github.com/anthropics/claude-plugins-official/blob/main/plugins/feature-dev/agents/code-reviewer.md and https://github.com/obra/superpowers/blob/main/skills/requesting-code-review/ (SKILL.md, code-reviewer.md) | Apache-2.0 — Copyright Anthropic, PBC; and MIT — Copyright (c) 2025 Jesse Vincent |
| `frontend-design` | https://github.com/anthropics/skills/blob/main/skills/frontend-design/SKILL.md | Apache-2.0 — Copyright 2026 Anthropic, PBC. |
| `webapp-testing` | https://github.com/anthropics/skills/blob/main/skills/webapp-testing/SKILL.md and scripts/with_server.py (script redistributed with a modification notice) | Apache-2.0 — Copyright 2026 Anthropic, PBC. |
| `api-design` | https://github.com/anthropics/skills/blob/main/skills/mcp-builder/ (SKILL.md, reference/mcp_best_practices.md, reference/evaluation.md) — ideas adapted from MCP tools to HTTP APIs | Apache-2.0 — Copyright 2026 Anthropic, PBC. |
| `ux-critique` | https://github.com/pbakaus/impeccable/blob/main/skill/reference/ (critique.md, critique-rubric.md, audit.md) | Apache-2.0 — Copyright 2025 Paul Bakaus. Impeccable's own credit: "Impeccable started from Anthropic's frontend-design skill" (https://github.com/anthropics/skills/tree/main/skills/frontend-design). |
| `research` | https://github.com/anthropics/claude-cookbooks/blob/main/patterns/agents/prompts/ (research_lead_agent.md, research_subagent.md) | MIT — Copyright (c) 2023 Anthropic |

## Roles

| File | Source | License and copyright |
|---|---|---|
| `roles/architect` | https://github.com/anthropics/claude-plugins-official/blob/main/plugins/feature-dev/agents/code-architect.md | Apache-2.0 — Copyright Anthropic, PBC |
| `roles/evaluator` | https://github.com/anthropics/claude-plugins-official/blob/main/plugins/feature-dev/agents/code-reviewer.md | Apache-2.0 — Copyright Anthropic, PBC |
| `roles/* (role boundaries only, no text copied)` | https://github.com/wshobson/agents | MIT — Copyright (c) 2024 Seth Hobson |

## Original RUI skills (Proprietary-RUI, not adapted)

`escalation-ladder`, `handoff-contract`, `devops-local`, `doc-coauthoring`, `fastapi`, `node-express`, `react-vite`.

`doc-coauthoring` shares its name and general idea with a skill in anthropics/skills, but that skill ships without a license file, so no text was copied; RUI's version is written from scratch. `handoff-contract` follows ideas from Anthropic's engineering post "How we built our multi-agent research system", in RUI's own words.

## MIT License

Applies to the files credited to obra/superpowers (Copyright (c) 2025 Jesse Vincent), anthropics/claude-cookbooks (Copyright (c) 2023 Anthropic) and wshobson/agents (Copyright (c) 2024 Seth Hobson). Each copyright line above is part of this notice.

```
MIT License

Copyright (c) <year> <copyright holders>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Apache License 2.0

Applies to the files credited to anthropics/skills (Copyright 2026 Anthropic, PBC.), anthropics/claude-plugins-official (Copyright Anthropic, PBC) and pbakaus/impeccable (Copyright 2025 Paul Bakaus).

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright 2026 Anthropic, PBC.

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
```
