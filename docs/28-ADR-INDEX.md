# 28 --- Architecture Decision Record Index

## Purpose

Accepted ADRs are authoritative technical decisions until explicitly
superseded.

## Accepted ADRs

  -----------------------------------------------------------------------
  ADR                     Decision                Status
  ----------------------- ----------------------- -----------------------
  ADR-001                 Modular Monolith        Accepted
                          application baseline    (2026-09-28,
                          with concrete           STEP-001)
                          repository, crate,
                          contract, worker,
                          WebSocket, document,
                          storage and testing
                          boundaries

  ADR-002                 Server-side revocable   Accepted
                          sessions, secure        
                          cookies, Argon2id       

  ADR-003                 Double-entry-inspired   Accepted
                          immutable financial     
                          ledger                  

  ADR-004                 PostgreSQL NUMERIC +    Accepted
                          exact decimals + typed  
                          value codes             

  ADR-005                 WebSocket real-time     Accepted
                          transport               

  ADR-006                 Transactions + scoped   Accepted
                          locking + optimistic    
                          concurrency +           
                          idempotency             

  ADR-007                 PostgreSQL metadata +   Accepted
                          private S3-compatible   
                          object storage          

  ADR-008                 Central typed/versioned Accepted
                          document rendering      
                          architecture            

  ADR-009                 PostgreSQL durable      Accepted
                          jobs + Rust worker +    
                          Transactional Outbox    

  ADR-010                 Cursor-first            Accepted
                          standardized pagination 

  ADR-011                 Structured              Accepted
                          observability +         
                          correlation +           
                          health/readiness        

  ADR-012                 Docker Compose          Accepted
                          single-node/VDS initial 
                          production topology     
  -----------------------------------------------------------------------

## Reserved ADR-001

ADR-001 was intentionally reserved in the v0.7 specification package for
the repository/application architectural baseline (Modular Monolith). It
was written and accepted during the STEP-001 repository architecture
review on 2026-09-28 (see
`docs/adr/ADR-001-APPLICATION-ARCHITECTURE-BASELINE.md`); it was not
retroactively fabricated.

## Rule

Agents must read every Accepted ADR relevant to their STEP.

An Accepted ADR may be changed only by: 1. explicit architecture review;
2. a new/superseding ADR; 3. documented migration consequences; 4.
updated tests/specifications.

Agents must not silently replace an Accepted decision.
