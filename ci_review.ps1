Write-Host "🌌 Nova CI: Initiating Autonomous Code Review..." -ForegroundColor Cyan

# 1. Frontend Review: Code Style & Linter
Write-Host "`n🎨 Layer 1: Frontend Linter (ESLint & Prettier)..." -ForegroundColor Yellow
pnpm lint
if ($LASTEXITCODE -ne 0) { Write-Error "Frontend Linting failed!"; exit 1 }

# 2. Type Review: TypeScript Consistency
Write-Host "`n🛡️ Layer 2: TypeScript Integrity (vue-tsc)..." -ForegroundColor Yellow
pnpm vue-tsc --noEmit
if ($LASTEXITCODE -ne 0) { Write-Error "TypeScript Type Check failed!"; exit 1 }

# 3. Rust Review: Logic Integrity & Performance
Write-Host "`n⚙️ Layer 3: Rust Core Review (clippy & fmt)..." -ForegroundColor Yellow
cd src-tauri
cargo fmt --check
if ($LASTEXITCODE -ne 0) { Write-Error "Rust Formatting failed!"; exit 1 }
cargo clippy -- -D warnings
if ($LASTEXITCODE -ne 0) { Write-Error "Rust Logic Review (clippy) failed!"; exit 1 }
cd ..

Write-Host "`n✅ Nova CI: Review Complete. All systems are green." -ForegroundColor Green
