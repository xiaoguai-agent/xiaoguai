HELM_CHART_DIR := deploy/helm/xiaoguai
HELM_RELEASE   := xiaoguai
RENDERED_OUT   := /tmp/xiaoguai-rendered.yaml
KUBECONFORM_VER := v0.6.7

.PHONY: helm-test helm-lint helm-template helm-validate helm-all

## Run helm unit tests (requires helm-unittest plugin)
helm-test:
	helm unittest $(HELM_CHART_DIR)

## Lint the helm chart
helm-lint:
	helm lint $(HELM_CHART_DIR)

## Render manifests to $(RENDERED_OUT)
helm-template:
	helm template $(HELM_RELEASE) $(HELM_CHART_DIR) \
		-f $(HELM_CHART_DIR)/values.yaml \
		> $(RENDERED_OUT)
	@echo "Rendered $$(wc -l < $(RENDERED_OUT)) lines → $(RENDERED_OUT)"

## Validate rendered manifests with kubeconform (installs if missing)
helm-validate: helm-template
	@command -v kubeconform >/dev/null 2>&1 || { \
		echo "Installing kubeconform $(KUBECONFORM_VER)..."; \
		curl -sSL \
			https://github.com/yannh/kubeconform/releases/download/$(KUBECONFORM_VER)/kubeconform-linux-amd64.tar.gz \
			| tar -xz -C /usr/local/bin kubeconform; \
	}
	kubeconform \
		-strict \
		-summary \
		-kubernetes-version 1.31.0 \
		-schema-location default \
		-schema-location 'https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json' \
		$(RENDERED_OUT)

## Run the full helm CI suite locally: test + lint + template + validate
helm-all: helm-test helm-lint helm-template helm-validate
	@echo "All helm checks passed."
