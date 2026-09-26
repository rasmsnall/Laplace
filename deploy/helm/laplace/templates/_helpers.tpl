{{- define "laplace.name" -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "laplace.labels" -}}
app.kubernetes.io/name: laplace
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "laplace.selector" -}}
app.kubernetes.io/name: laplace
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "laplace.image" -}}
{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}
{{- end -}}

{{/*
the PG variables for one database role, when database.host is set; the password comes
straight from the secret that holds it, such as one cloudnativepg made
*/}}
{{- define "laplace.database" -}}
{{- $db := .root.Values.database -}}
- name: PGHOST
  value: {{ $db.host | quote }}
- name: PGPORT
  value: {{ $db.port | quote }}
- name: PGDATABASE
  value: {{ $db.name | quote }}
- name: PGSSLMODE
  value: {{ $db.sslmode | quote }}
- name: PGUSER
  value: {{ .role.user | quote }}
- name: PGPASSWORD
  valueFrom:
    secretKeyRef:
      name: {{ required (printf "database.%s.passwordSecret is needed with database.host" .name) .role.passwordSecret }}
      key: {{ .role.passwordKey }}
{{- end -}}

{{/* the same locked-down settings for the app and the migration job */}}
{{- define "laplace.podSecurity" -}}
runAsNonRoot: true
runAsUser: 10001
runAsGroup: 10001
fsGroup: 10001
seccompProfile:
  type: RuntimeDefault
{{- end -}}

{{- define "laplace.containerSecurity" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
capabilities:
  drop: ["ALL"]
{{- end -}}
