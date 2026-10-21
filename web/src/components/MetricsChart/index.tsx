import ReactECharts from 'echarts-for-react';
import type { MetricsSnapshot } from '../../types';
import { useI18n } from '../../i18n';

interface MetricsChartProps {
  data: MetricsSnapshot[];
  type: 'cpu' | 'memory' | 'qps' | 'bandwidth' | 'connections';
  height?: number;
}

export default function MetricsChart({ data, type, height = 200 }: MetricsChartProps) {
  const { t } = useI18n();
  const sentLabel = t('traffic.metrics.sent');
  const receivedLabel = t('traffic.metrics.received');
  const timestamps = data.map(d => new Date(d.timestamp).toLocaleTimeString());

  const getOption = () => {
    switch (type) {
      case 'cpu':
        return {
          tooltip: { trigger: 'axis' },
          xAxis: { type: 'category', data: timestamps },
          yAxis: { type: 'value', max: 100, axisLabel: { formatter: '{value}%' } },
          series: [{
            name: 'CPU',
            type: 'line',
            smooth: true,
            data: data.map(d => d.cpu_usage.toFixed(1)),
            areaStyle: { opacity: 0.3 },
          }],
          grid: { left: 50, right: 20, top: 20, bottom: 30 },
        };
      
      case 'memory':
        return {
          tooltip: { trigger: 'axis' },
          xAxis: { type: 'category', data: timestamps },
          yAxis: { type: 'value', axisLabel: { formatter: (v: number) => `${(v / 1024 / 1024).toFixed(0)}MB` } },
          series: [{
            name: t('traffic.metrics.memory'),
            type: 'line',
            smooth: true,
            data: data.map(d => d.memory_used),
            areaStyle: { opacity: 0.3 },
          }],
          grid: { left: 60, right: 20, top: 20, bottom: 30 },
        };
      
      case 'qps':
        return {
          tooltip: { trigger: 'axis' },
          xAxis: { type: 'category', data: timestamps },
          yAxis: { type: 'value' },
          series: [{
            name: 'QPS',
            type: 'line',
            smooth: true,
            data: data.map(d => d.qps.toFixed(2)),
            areaStyle: { opacity: 0.3 },
          }],
          grid: { left: 50, right: 20, top: 20, bottom: 30 },
        };

      case 'bandwidth':
        return {
          tooltip: { trigger: 'axis', formatter: (params: { seriesName: string; value: number }[]) => {
            return params.map(p => `${p.seriesName}: ${formatBytes(p.value)}/s`).join('<br/>');
          }},
          legend: { data: [sentLabel, receivedLabel] },
          xAxis: { type: 'category', data: timestamps },
          yAxis: { type: 'value', axisLabel: { formatter: (v: number) => `${formatBytes(v)}/s` } },
          series: [
            {
              name: sentLabel,
              type: 'line',
              smooth: true,
              data: data.map(d => d.bytes_sent_rate),
            },
            {
              name: receivedLabel,
              type: 'line',
              smooth: true,
              data: data.map(d => d.bytes_received_rate),
            },
          ],
          grid: { left: 70, right: 20, top: 30, bottom: 30 },
        };

      case 'connections':
        return {
          tooltip: { trigger: 'axis' },
          xAxis: { type: 'category', data: timestamps },
          yAxis: { type: 'value' },
          series: [{
            name: t('traffic.metrics.connections'),
            type: 'line',
            smooth: true,
            data: data.map(d => d.active_connections),
            areaStyle: { opacity: 0.3 },
          }],
          grid: { left: 50, right: 20, top: 20, bottom: 30 },
        };

      default:
        return {};
    }
  };

  return <ReactECharts option={getOption()} style={{ height }} />;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(1)}${sizes[i]}`;
}
