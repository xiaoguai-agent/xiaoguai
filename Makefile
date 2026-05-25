.PHONY: validate validate-packs validate-watchers validate-hotl-policies validate-recipes

## validate: Run all 4 manifest validators locally (mirrors CI jobs)
validate: validate-packs validate-watchers validate-hotl-policies validate-recipes
	@echo "All manifest validators passed."

validate-packs:
	@echo "==> Validating packs..."
	bash scripts/validate-pack.sh

validate-watchers:
	@echo "==> Validating watchers..."
	bash scripts/validate-watcher.sh

validate-hotl-policies:
	@echo "==> Validating HOTL policies..."
	bash scripts/validate-hotl-policy.sh

validate-recipes:
	@echo "==> Validating recipes..."
	bash scripts/validate-recipe.sh
