{{/*
Expand the name of the chart.
*/}}
{{- define "xiaoguai-observability.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "xiaoguai-observability.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name (include "xiaoguai-observability.name" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{/*
Chart label (name-version, safe for label values).
*/}}
{{- define "xiaoguai-observability.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels — applied to every resource.
*/}}
{{- define "xiaoguai-observability.labels" -}}
helm.sh/chart: {{ include "xiaoguai-observability.chart" . }}
{{ include "xiaoguai-observability.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels — used in matchLabels.
*/}}
{{- define "xiaoguai-observability.selectorLabels" -}}
app.kubernetes.io/name: {{ include "xiaoguai-observability.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Namespace where xiaoguai-core is deployed (from global.xiaoguaiNamespace).
*/}}
{{- define "xiaoguai-observability.xiaoguaiNamespace" -}}
{{- default "default" .Values.global.xiaoguaiNamespace }}
{{- end }}

{{/*
Release name of the main xiaoguai chart (from global.xiaoguaiRelease).
*/}}
{{- define "xiaoguai-observability.xiaoguaiRelease" -}}
{{- default "xiaoguai" .Values.global.xiaoguaiRelease }}
{{- end }}

{{/*
Loki URL within the cluster — used by Grafana datasource.
Loki is deployed as <release>-loki in the same namespace as this chart.
*/}}
{{- define "xiaoguai-observability.lokiUrl" -}}
{{- printf "http://%s-loki:3100" .Release.Name }}
{{- end }}

{{/*
Tempo URL within the cluster — used by Grafana datasource.
Tempo is deployed as <release>-tempo in the same namespace as this chart.
*/}}
{{- define "xiaoguai-observability.tempoUrl" -}}
{{- printf "http://%s-tempo:3100" .Release.Name }}
{{- end }}

{{/*
Prometheus URL within the cluster — kube-prometheus-stack exposes it as
<release>-kube-prometheus-stack-prometheus on port 9090.
*/}}
{{- define "xiaoguai-observability.prometheusUrl" -}}
{{- printf "http://%s-kube-prometheus-stack-prometheus:9090" .Release.Name }}
{{- end }}
