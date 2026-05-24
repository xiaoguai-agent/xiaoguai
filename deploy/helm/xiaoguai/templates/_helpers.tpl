{{/*
Expand the name of the chart.
*/}}
{{- define "xiaoguai.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "xiaoguai.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name (include "xiaoguai.name" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{/*
Chart label (name-version, safe for label values).
*/}}
{{- define "xiaoguai.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels — applied to every resource.
*/}}
{{- define "xiaoguai.labels" -}}
helm.sh/chart: {{ include "xiaoguai.chart" . }}
{{ include "xiaoguai.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- with .Values.commonLabels }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Selector labels — used in matchLabels and Service selectors.
*/}}
{{- define "xiaoguai.selectorLabels" -}}
app.kubernetes.io/name: {{ include "xiaoguai.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Full image reference — repository:tag (tag defaults to appVersion).
*/}}
{{- define "xiaoguai.image" -}}
{{ .Values.image.repository }}:{{ default .Chart.AppVersion .Values.image.tag }}
{{- end }}

{{/*
Name of the ConfigMap holding non-secret config.
*/}}
{{- define "xiaoguai.configmapName" -}}
{{ include "xiaoguai.fullname" . }}-config
{{- end }}

{{/*
Name of the chart-managed Secret (only created when createSecret=true).
*/}}
{{- define "xiaoguai.secretName" -}}
{{ include "xiaoguai.fullname" . }}-credentials
{{- end }}

{{/*
Headless service name.
*/}}
{{- define "xiaoguai.headlessServiceName" -}}
{{ include "xiaoguai.fullname" . }}-headless
{{- end }}
