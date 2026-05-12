import { Card, Col, Row, Space, Typography, Tag, Button, Image, Divider } from "antd";
import {
  CheckOutlined,
  CloseOutlined,
  DownloadOutlined,
  QrcodeOutlined,
  SafetyCertificateOutlined,
} from "@ant-design/icons";
import type { CertInfo } from "../../../api/cert";

const { Text } = Typography;

export interface CertificateTabProps {
  certInfo: CertInfo | null;
  getCertDownloadUrl: () => string;
  getCertQRCodeUrl: (ip?: string) => string;
}

export default function CertificateTab({
  certInfo,
  getCertDownloadUrl,
  getCertQRCodeUrl,
}: CertificateTabProps) {
  const certStatus = certInfo?.status ?? "unknown";
  const certStatusLabel = certInfo?.status_label ?? "Check failed";
  const certStatusColor =
    certStatus === "installed_and_trusted"
      ? "green"
      : certStatus === "installed_not_trusted"
        ? "orange"
        : certStatus === "not_installed"
          ? "red"
          : "default";
  const certStatusIcon =
    certStatus === "installed_and_trusted" ? <CheckOutlined /> : <CloseOutlined />;

  return (
    <Row gutter={[16, 16]} data-testid="settings-certificate-tab">
      <Col xs={24}>
        <Card
          title={
            <Space>
              <SafetyCertificateOutlined />
              <span>CA Certificate</span>
            </Space>
          }
          size="small"
        >
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            <Row justify="space-between" align="middle">
              <Col>
                <Text>Certificate Status</Text>
              </Col>
              <Col>
                <Tag color={certStatusColor} icon={certStatusIcon}>
                  {certStatusLabel}
                </Tag>
              </Col>
            </Row>

            <Divider style={{ margin: "8px 0" }} />

            <Text type="secondary" style={{ fontSize: 12 }}>
              {certInfo?.status_message ??
                "Unable to verify whether the CA certificate is installed and trusted."}
            </Text>

            <Button
              type="primary"
              icon={<DownloadOutlined />}
              href={getCertDownloadUrl()}
              download="bifrost-ca.crt"
              disabled={!certInfo?.available}
              block
              data-testid="settings-certificate-download"
            >
              Download CA Certificate
            </Button>

            <Text type="secondary" style={{ fontSize: 12 }}>
              {certInfo?.available
                ? "Download the CA certificate file and install it as a trusted root CA on your device to enable HTTPS inspection."
                : "The CA certificate file is not available yet, so it cannot be downloaded or trusted on this device."}
            </Text>
          </Space>
        </Card>
      </Col>

      <Col xs={24}>
        <Card
          title={
            <Space>
              <QrcodeOutlined />
              <span>Mobile Installation</span>
            </Space>
          }
          size="small"
        >
          {certInfo?.available && certInfo.download_urls.length > 0 ? (
            <>
              <Text
                type="secondary"
                style={{
                  fontSize: 12,
                  display: "block",
                  marginBottom: 12,
                }}
              >
                Scan with your mobile device to download and install the CA certificate
              </Text>
              <Row gutter={[16, 16]} justify="start">
                {certInfo.download_urls.map((url, index) => {
                  const ip = certInfo.local_ips[index] || "";
                  return (
                    <Col key={ip || index}>
                      <div style={{ textAlign: "center" }}>
                        <Image
                          src={getCertQRCodeUrl(ip || undefined)}
                          alt={`Certificate QR Code - ${ip}`}
                          width={120}
                          height={120}
                          preview={{
                            mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
                          }}
                          fallback="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mN8/+F9PQAJpAN4pokyXwAAAABJRU5ErkJggg=="
                          data-testid={
                            index === 0
                              ? "settings-certificate-qrcode"
                              : `settings-certificate-qrcode-${ip}`
                          }
                        />
                        <div style={{ marginTop: 4 }}>
                          <a
                            href={url}
                            target="_blank"
                            rel="noreferrer"
                            style={{ fontSize: 12 }}
                          >
                            {url}
                          </a>
                        </div>
                      </div>
                    </Col>
                  );
                })}
              </Row>
            </>
          ) : (
            <Text type="secondary">
              {certInfo?.available
                ? "No download URLs available"
                : "QR code not available"}
            </Text>
          )}
        </Card>
      </Col>
    </Row>
  );
}
