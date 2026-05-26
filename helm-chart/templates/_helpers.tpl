{{/*
  Expand the name of the chart.
*/}}
{{- define "autoinference.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
  Fully qualified app name. Truncated to 63 chars (K8s label limit).
*/}}
{{- define "autoinference.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
  Chart version string for the chart label.
*/}}
{{- define "autoinference.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
  Common labels.
*/}}
{{- define "autoinference.labels" -}}
helm.sh/chart: {{ include "autoinference.chart" . }}
{{ include "autoinference.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: autoinference
{{- end -}}

{{/*
  Selector labels.
*/}}
{{- define "autoinference.selectorLabels" -}}
app.kubernetes.io/name: {{ include "autoinference.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/*
  Create the name of the service account to use.
*/}}
{{- define "autoinference.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "autoinference.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}
